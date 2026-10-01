//! Running `virsh`, working around a libvirt session daemon restart race
//!
//! This works around a libvirt bug, present from at least v10.0.0 through
//! v12.8.0 (current master as of 2026-10). For `qemu:///session`, the libvirt
//! client auto-spawns a per-user daemon (`libvirtd` or `virtqemud`) with
//! `--timeout=120`, which exits after two idle minutes. On exit the daemon
//! removes its socket first (`virNetDaemonRun()` closes its servers) and
//! releases its pidfile only once its drivers have shut down, at the end of
//! `main()` in `src/remote/remote_daemon.c`. A client connecting in between
//! gets ENOENT and, in `virNetSocketNewConnectUNIX()`
//! (`src/rpc/virnetsocket.c`), spawns a new daemon once; that daemon exits
//! immediately because the pidfile is still held, and the client then waits
//! five seconds for a socket nobody will create and fails with
//! `Failed to connect socket to '.../libvirt-sock': No such file or directory`.
//! See <https://github.com/bootc-dev/bootc/issues/1843>.
//!
//! Upstream bug not yet filed; draft report with a reproducer:
//! <https://gist.github.com/cgwalters-bot/47081fd1fdf2333b3fe83c79f4a11887>.
//! Remove this retry once the libvirt versions bcvk supports include a fix
//! (link it here when one lands).
//!
//! When virsh fails to connect it has not run its command, so retrying is
//! always safe. Every retry spawns the daemon again, which succeeds once the
//! old one has gone.

use std::ffi::OsStr;
use std::io;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// How long to keep retrying a failed connection. libvirt gives an exiting
/// daemon up to 30 seconds to shut down, and each virsh attempt itself waits
/// up to 5 seconds for the socket of the daemon it spawned.
const CONNECT_RETRY_TIMEOUT: Duration = Duration::from_secs(60);

/// First delay between attempts; it doubles up to [`MAX_BACKOFF`].
const INITIAL_BACKOFF: Duration = Duration::from_millis(250);

/// Longest delay between attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(4);

/// What virsh prints when opening the connection failed.
const CONNECT_FAILED: &str = "failed to connect to the hypervisor";

/// What libvirt prints when the daemon's socket can't be connected to.
const SOCKET_CONNECT_FAILED: &str = "Failed to connect socket to";

/// Socket errors that mean the session daemon is exiting or not listening
/// yet, as opposed to e.g. EACCES.
const TRANSIENT_SOCKET_ERRORS: &[&str] = &["No such file or directory", "Connection refused"];

/// Errors from a daemon that accepted the connection while it was exiting.
const TRANSIENT_STREAM_ERRORS: &[&str] =
    &["End of file while reading data", "Connection reset by peer"];

/// A `virsh` invocation with an optional connection URI.
///
/// Unlike a plain [`Command`], [`VirshCommand::output`] retries when virsh
/// could not connect to a session daemon that is restarting. It deliberately
/// doesn't deref to [`Command`], so a call chain can't skip the retry.
#[derive(Debug)]
pub struct VirshCommand {
    cmd: Command,
    retry_connect: bool,
}

impl VirshCommand {
    /// Create a virsh command, connecting to `connect_uri` or virsh's default.
    pub fn new(connect_uri: Option<&str>) -> Self {
        let mut cmd = Command::new("virsh");
        // We match on virsh's error messages, so they must not be translated.
        cmd.env("LC_ALL", "C");
        if let Some(uri) = connect_uri {
            cmd.arg("-c").arg(uri);
        }
        let default_uri = std::env::var("LIBVIRT_DEFAULT_URI").ok();
        let retry_connect = uses_session_daemon(
            connect_uri.or(default_uri.as_deref()),
            rustix::process::geteuid().is_root(),
        );
        Self { cmd, retry_connect }
    }

    /// Add arguments, like [`Command::args`].
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.cmd.args(args);
        self
    }

    /// Run virsh and collect its output, like [`Command::output`], retrying
    /// while the session daemon can't be reached.
    pub fn output(&mut self) -> io::Result<Output> {
        if !self.retry_connect {
            return self.cmd.output();
        }
        let cmd = &mut self.cmd;
        output_with_retry(|| cmd.output(), CONNECT_RETRY_TIMEOUT, INITIAL_BACKOFF)
    }
}

/// Whether connecting to `uri` (`None` meaning libvirt's built-in default)
/// goes through an auto-spawned per-user daemon.
fn uses_session_daemon(uri: Option<&str>, is_root: bool) -> bool {
    match uri {
        Some(uri) => {
            let without_query = uri.split_once('?').map_or(uri, |(base, _)| base);
            without_query.ends_with("/session")
        }
        // Without a URI, libvirt picks qemu:///session for unprivileged users.
        None => !is_root,
    }
}

/// Whether virsh failed because the session daemon was exiting or not up yet.
fn is_transient_connect_failure(output: &Output) -> bool {
    if output.status.success() {
        return false;
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr.contains(CONNECT_FAILED)
        && stderr.lines().any(|line| {
            let socket_gone = line.contains(SOCKET_CONNECT_FAILED)
                && TRANSIENT_SOCKET_ERRORS.iter().any(|e| line.ends_with(e));
            socket_gone || TRANSIENT_STREAM_ERRORS.iter().any(|e| line.contains(e))
        })
}

/// Call `run` until it succeeds, fails for another reason than a transient
/// connection failure, or `timeout` has passed; the last output is returned.
fn output_with_retry(
    mut run: impl FnMut() -> io::Result<Output>,
    timeout: Duration,
    initial_backoff: Duration,
) -> io::Result<Output> {
    let deadline = Instant::now() + timeout;
    let mut backoff = initial_backoff;
    let mut attempt = 1u32;
    loop {
        let output = run()?;
        if !is_transient_connect_failure(&output) || Instant::now() >= deadline {
            return Ok(output);
        }
        tracing::debug!(
            "virsh could not connect to the libvirt session daemon (attempt {attempt}), retrying in {backoff:?}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(MAX_BACKOFF);
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    const ENOENT_STDERR: &str = "error: failed to connect to the hypervisor\nerror: Failed to connect socket to '/run/user/1001/libvirt/libvirt-sock': No such file or directory\n";

    fn output(code: i32, stderr: &str) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn test_uses_session_daemon() {
        let cases = [
            (Some("qemu:///session"), false, true),
            (Some("qemu:///session?socket=/tmp/sock"), false, true),
            (Some("qemu:///system"), false, false),
            (Some("qemu+ssh://host/system"), false, false),
            (None, false, true),
            (None, true, false),
        ];
        for (uri, is_root, expected) in cases {
            assert_eq!(
                uses_session_daemon(uri, is_root),
                expected,
                "uri={uri:?} is_root={is_root}"
            );
        }
    }

    #[test]
    fn test_is_transient_connect_failure() {
        let cases = [
            (1, ENOENT_STDERR, true),
            (
                1,
                "error: failed to connect to the hypervisor\nerror: Failed to connect socket to '/run/user/1000/libvirt/virtqemud-sock': Connection refused\n",
                true,
            ),
            (
                1,
                "error: failed to connect to the hypervisor\nerror: End of file while reading data: Input/output error\n",
                true,
            ),
            // Not a connection problem: the command itself failed
            (1, "error: failed to get domain 'foo'\n", false),
            // A missing daemon binary won't fix itself
            (
                1,
                "error: failed to connect to the hypervisor\nerror: binary 'virtqemud' does not exist in $PATH: No such file or directory\n",
                false,
            ),
            (
                1,
                "error: failed to connect to the hypervisor\nerror: Failed to connect socket to '/run/user/1000/libvirt/libvirt-sock': Permission denied\n",
                false,
            ),
            (0, ENOENT_STDERR, false),
        ];
        for (code, stderr, expected) in cases {
            assert_eq!(
                is_transient_connect_failure(&output(code, stderr)),
                expected,
                "code={code} stderr={stderr:?}"
            );
        }
    }

    #[test]
    fn test_output_with_retry() {
        // (failures before success, timeout, expected attempts, expected success)
        let cases = [
            (0, Duration::from_secs(60), 1, true),
            (3, Duration::from_secs(60), 4, true),
            // Once the deadline has passed the last failure is returned
            (u32::MAX, Duration::ZERO, 1, false),
        ];
        for (failures, timeout, expected_attempts, expected_success) in cases {
            let mut attempts = 0u32;
            let out = output_with_retry(
                || {
                    attempts += 1;
                    Ok(if attempts > failures {
                        output(0, "")
                    } else {
                        output(1, ENOENT_STDERR)
                    })
                },
                timeout,
                Duration::ZERO,
            )
            .unwrap();
            assert_eq!(attempts, expected_attempts, "failures={failures}");
            assert_eq!(out.status.success(), expected_success);
        }
    }

    #[test]
    fn test_output_with_retry_other_failure() {
        let mut attempts = 0;
        let out = output_with_retry(
            || {
                attempts += 1;
                Ok(output(1, "error: failed to get domain 'foo'\n"))
            },
            Duration::from_secs(60),
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(attempts, 1);
        assert!(!out.status.success());
    }
}
