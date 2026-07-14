use color_eyre::eyre::{eyre, Context as _};
use color_eyre::Result;
use indicatif::ProgressBar;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tracing::debug;

use crate::run_ephemeral::{run_detached, RunEphemeralOpts};
use crate::ssh;
use crate::supervisor_status::{SupervisorState, SupervisorStatus};

/// Container state from podman inspect
#[derive(Debug, serde::Deserialize)]
struct ContainerInspect {
    #[serde(rename = "State")]
    state: ContainerState,
}

#[derive(Debug, serde::Deserialize)]
struct ContainerState {
    #[serde(rename = "Status")]
    status: String,
    #[serde(rename = "ExitCode")]
    exit_code: i32,
    #[serde(rename = "Error")]
    error: Option<String>,
}

/// Fetch and display container logs to help diagnose startup failures
fn show_container_logs(container_name: &str) {
    debug!("Fetching container logs for {}", container_name);

    // Get container state in a single inspect call
    let state = Command::new("podman")
        .args(["inspect", "--", container_name])
        .output()
        .ok()
        .and_then(|output| {
            serde_json::from_slice::<Vec<ContainerInspect>>(&output.stdout)
                .ok()
                .and_then(|mut inspects| inspects.pop())
                .map(|inspect| inspect.state)
        });

    if let Some(ref s) = state {
        eprint!(
            "\nContainer state: {} (exit code: {})",
            s.status, s.exit_code
        );
        if let Some(ref err) = s.error {
            if !err.is_empty() {
                eprint!(" - Error: {}", err);
            }
        }
        eprintln!();

        // Provide helpful hints for common exit codes
        match s.exit_code {
            127 => {
                eprintln!("\nNote: Exit code 127 typically means 'command not found'.");
                eprintln!("This container image may not be a valid bootc image.");
                eprintln!("Bootc images must have systemd and kernel modules in /usr/lib/modules.");
            }
            126 => {
                eprintln!("\nNote: Exit code 126 typically means 'permission denied' or file not executable.");
            }
            _ => {}
        }
    }

    let output = match Command::new("podman")
        .args(["logs", "--", container_name])
        .stderr(Stdio::inherit())
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            eprintln!("Failed to fetch container logs: {}", e);
            return;
        }
    };

    let logs = String::from_utf8_lossy(&output.stdout);
    if !logs.trim().is_empty() {
        eprintln!("\nContainer logs:");
        eprintln!("----------------------------------------");
        for line in logs.lines() {
            eprintln!("{}", line);
        }
        eprintln!("----------------------------------------\n");
    } else {
        eprintln!("(Container produced no output)");
    }
}

/// RAII guard for ephemeral container cleanup
/// Ensures container is removed when dropped, even on error paths
struct ContainerCleanup {
    container_id: String,
}

impl ContainerCleanup {
    fn new(container_id: String) -> Self {
        Self { container_id }
    }
}

impl Drop for ContainerCleanup {
    fn drop(&mut self) {
        debug!("Cleaning up ephemeral container {}", self.container_id);
        let result = Command::new("podman")
            .args(["rm", "-f", "--", &self.container_id])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        if let Err(e) = result {
            tracing::warn!("Failed to remove container {}: {}", self.container_id, e);
        }
    }
}

/// Timeout waiting for connection
pub(crate) const SSH_TIMEOUT: std::time::Duration = const { Duration::from_secs(240) };

#[derive(Debug, clap::Parser, serde::Serialize, serde::Deserialize)]
pub struct RunEphemeralSshOpts {
    #[command(flatten)]
    pub run_opts: RunEphemeralOpts,

    /// SSH command to execute (optional, defaults to interactive shell)
    #[arg(trailing_var_arg = true)]
    pub ssh_args: Vec<String>,
}

/// Check if container is running
fn is_container_running(container_name: &str) -> Result<bool> {
    let output = Command::new("podman")
        .args([
            "inspect",
            "--format",
            "{{.State.Status}}",
            "--",
            container_name,
        ])
        .output()
        .context("Failed to inspect container state")?;

    let state = String::from_utf8_lossy(&output.stdout);
    Ok(state.trim() == "running")
}

/// Spawn the status monitor subprocess and return the child process.
///
/// The monitor watches /run/supervisor-status.json inside the container via
/// inotify and streams JSON status lines to stdout.
fn spawn_status_monitor(container_name: &str) -> Result<std::process::Child> {
    let mut cmd = Command::new("podman");
    cmd.args([
        "exec",
        "--",
        container_name,
        "/var/lib/bcvk/entrypoint",
        "monitor-status",
    ]);
    // SAFETY: This API is safe to call in a forked child.
    #[allow(unsafe_code)]
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::TERM))
                .map_err(Into::into)
        });
    }
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("Failed to start status monitor")
}

/// Events from the concurrent monitor and SSH polling threads.
enum ReadinessEvent {
    MonitorLine(std::io::Result<String>),
    SshReady,
}

/// Wait for SSH to be ready, using vsock boot progress and SSH polling concurrently.
///
/// Starts the vsock-based status monitor alongside SSH connectivity polling,
/// each in their own thread. Both write to a shared channel; the main thread
/// blocks on recv_timeout() so no CPU is burned polling.
pub fn wait_for_ssh_ready(
    container_name: &str,
    timeout: Option<Duration>,
    progress: ProgressBar,
) -> Result<(std::time::Duration, ProgressBar)> {
    let timeout = timeout.unwrap_or(SSH_TIMEOUT);
    let start = Instant::now();

    if !is_container_running(container_name)? {
        progress.finish_and_clear();
        show_container_logs(container_name);
        return Err(eyre!("Container exited before SSH became available"));
    }

    let (tx, rx) = std::sync::mpsc::channel::<ReadinessEvent>();

    // Spawn the vsock status monitor reader thread.
    let mut monitor_child = match spawn_status_monitor(container_name) {
        Ok(child) => Some(child),
        Err(e) => {
            debug!("Status monitor failed to start, using SSH polling only: {e}");
            None
        }
    };
    if let Some(ref mut child) = monitor_child {
        let stdout = child.stdout.take().unwrap();
        let monitor_tx = tx.clone();
        std::thread::spawn(move || {
            let reader = std::io::BufReader::new(stdout);
            for line in std::io::BufRead::lines(reader) {
                if monitor_tx.send(ReadinessEvent::MonitorLine(line)).is_err() {
                    break;
                }
            }
        });
    }

    // Spawn SSH polling thread.
    let ssh_container = container_name.to_string();
    let ssh_tx = tx.clone();
    std::thread::spawn(move || {
        let ssh_options = crate::ssh::SshConnectionOptions::for_connectivity_test();
        loop {
            let status =
                crate::ssh::connect(&ssh_container, vec!["true".to_string()], &ssh_options);
            if matches!(status, Ok(ref s) if s.success()) {
                let _ = ssh_tx.send(ReadinessEvent::SshReady);
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });

    // Drop our copy so the channel disconnects when both threads exit.
    drop(tx);

    debug!(
        "Waiting for VM readiness (timeout: {}s), polling SSH and monitoring vsock concurrently",
        timeout.as_secs()
    );

    loop {
        let remaining = timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            if let Some(ref mut child) = monitor_child {
                let _ = child.kill();
            }
            progress.finish_and_clear();
            show_container_logs(container_name);
            return Err(eyre!(
                "Timeout waiting for readiness after {}s",
                timeout.as_secs()
            ));
        }

        let event = match rx.recv_timeout(remaining) {
            Ok(event) => event,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(ref mut child) = monitor_child {
                    let _ = child.kill();
                }
                progress.finish_and_clear();
                show_container_logs(container_name);
                return Err(eyre!("Both monitor and SSH polling stopped unexpectedly"));
            }
        };

        match event {
            ReadinessEvent::SshReady => {
                debug!("SSH ready after {}s", start.elapsed().as_secs());
                if let Some(ref mut child) = monitor_child {
                    let _ = child.kill();
                }
                return Ok((start.elapsed(), progress));
            }
            ReadinessEvent::MonitorLine(line) => match line {
                Ok(line) => {
                    if let Ok(status) = serde_json::from_str::<SupervisorStatus>(&line) {
                        debug!("Status update: {:?}", status.state);
                        if status.ssh_access {
                            if let Some(ref mut child) = monitor_child {
                                let _ = child.kill();
                            }
                            return Ok((start.elapsed(), progress));
                        }
                        if let Some(ref state) = status.state {
                            match state {
                                SupervisorState::Ready => {
                                    progress.set_message("Ready");
                                }
                                SupervisorState::ReachedTarget(target) => {
                                    progress.set_message(format!("Reached target {}", target));
                                }
                                SupervisorState::WaitingForSystemd => {
                                    progress.set_message("Waiting for systemd...");
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!("Monitor read error: {e}");
                }
            },
        }
    }
}

/// Run an ephemeral pod and immediately SSH into it, with lifecycle binding
pub fn run_ephemeral_ssh(opts: RunEphemeralSshOpts) -> Result<()> {
    // Start the ephemeral pod in detached mode with SSH enabled
    let mut ephemeral_opts = opts.run_opts.clone();
    ephemeral_opts.podman.detach = true;
    ephemeral_opts.common.ssh_keygen = true; // Enable SSH key generation and access

    debug!("Starting ephemeral VM...");
    let container_id = run_detached(ephemeral_opts)?;
    debug!("Ephemeral VM started with container ID: {}", container_id);

    // Create cleanup guard to ensure container removal on any exit path
    let _cleanup = ContainerCleanup::new(container_id.clone());

    // Use the container ID for SSH and cleanup
    let container_name = container_id;
    debug!("Using container ID: {}", container_name);

    let progress_bar = crate::boot_progress::create_boot_progress_bar();
    let (_duration, progress_bar) = wait_for_ssh_ready(&container_name, None, progress_bar)?;
    progress_bar.finish_and_clear();

    // Execute SSH connection directly (no thread needed for this)
    // This allows SSH output to be properly forwarded to stdout/stderr
    debug!("Connecting to SSH with args: {:?}", opts.ssh_args);
    let status = ssh::connect(
        &container_name,
        opts.ssh_args,
        &ssh::SshConnectionOptions::default(),
    )?;
    debug!("SSH connection completed");

    let exit_code = status.code().unwrap_or(1);
    debug!("SSH exit code: {}", exit_code);

    // Explicitly drop the cleanup guard before exit to ensure container removal
    // (std::process::exit doesn't run drop handlers)
    drop(_cleanup);

    // Exit with SSH client's exit code
    std::process::exit(exit_code);
}
