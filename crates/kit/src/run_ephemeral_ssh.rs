use color_eyre::eyre::{eyre, Context as _};
use color_eyre::Result;
use indicatif::ProgressBar;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tracing::debug;

use crate::run_ephemeral::{run_detached, RunEphemeralOpts};
use crate::ssh;
use crate::supervisor_status::{SupervisorState, SupervisorStatus};

#[derive(Debug, clap::Parser, serde::Serialize, serde::Deserialize)]
pub struct RunEphemeralSshOpts {
    #[command(flatten)]
    pub run_opts: RunEphemeralOpts,

    /// SSH command to execute (optional, defaults to interactive shell)
    #[arg(trailing_var_arg = true)]
    pub ssh_args: Vec<String>,
}

/// Wait for VM SSH availability using the supervisor status file
///
/// Monitors /run/supervisor-status.json inside the container for SSH.
/// Returns Ok(true) when systemd indicates ssh is probably ready.
/// Returns Ok(false) if we don't support systemd status notifications.
pub fn wait_for_vm_ssh(
    container_name: &str,
    timeout: Duration,
    progress: ProgressBar,
) -> Result<(bool, ProgressBar)> {
    debug!(
        "Waiting for VM readiness via supervisor status file (timeout: {}s)...",
        timeout.as_secs()
    );

    // Use the new monitor-status subcommand for efficient inotify-based monitoring
    let mut cmd = Command::new("podman");
    cmd.args([
        "exec",
        container_name,
        "/var/lib/bcvk/entrypoint",
        "monitor-status",
    ]);
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::TERM))
                .map_err(Into::into)
        });
    }
    let mut child = cmd
        .stdout(Stdio::piped())
        .spawn()
        .context("Failed to start status monitor")?;

    let stdout = child.stdout.take().unwrap();
    let reader = std::io::BufReader::new(stdout);

    // Read JSON lines from the monitor
    for line in std::io::BufRead::lines(reader) {
        let line = line.context("Reading monitor output")?;

        let status = serde_json::from_str::<SupervisorStatus>(&line)?;
        debug!("Status update: {:?}", status.state);

        if status.ssh_access {
            // End the monitor
            let _ = child.kill();
            return Ok((true, progress));
        }

        if let Some(state) = status.state {
            match state {
                SupervisorState::Ready => {
                    debug!("VM is ready!");
                    progress.set_message("Ready");
                }
                SupervisorState::ReachedTarget(ref target) => {
                    progress.set_message(format!("Reached target {}", target));
                    debug!("Boot progress: Reached {}", target);
                }
                SupervisorState::WaitingForSystemd => {
                    progress.set_message("Waiting for systemd...");
                    debug!("Waiting for systemd to initialize...");
                }
            }
        } else {
            debug!("Target does not support systemd readiness");
            return Ok((false, progress));
        }
    }

    let status = child.wait()?;
    Err(eyre!("Monitor process exited unexpectedly: {status:?}"))
}

/// Wait for SSH to be ready by polling SSH connection attempts
///
/// Attempts to connect to the VM via SSH until successful or timeout.
/// This is used as a fallback when systemd notification is not available.
pub fn wait_for_ssh_ready(
    container_name: &str,
    timeout: Duration,
    progress: ProgressBar,
) -> Result<ProgressBar> {
    let (_, progress) = wait_for_vm_ssh(container_name, timeout, progress)?;

    debug!(
        "Polling SSH connectivity (timeout: {}s)...",
        timeout.as_secs()
    );
    let start_time = Instant::now();

    // Use SSH options optimized for connectivity testing
    let ssh_options = crate::ssh::SshConnectionOptions::for_connectivity_test();

    let mut attempt = 0;
    while start_time.elapsed() < timeout {
        attempt += 1;
        progress.set_message(format!("Polling for SSH readiness (attempt {attempt})"));

        // Try to connect via SSH and run a simple command using the centralized SSH function
        let status = crate::ssh::connect_via_container_with_options(
            container_name,
            vec!["true".to_string()], // Just run 'true' to test connectivity
            &ssh_options,
        );

        if let Ok(exit_status) = status {
            if exit_status.success() {
                debug!("SSH connection successful, VM is ready");
                return Ok(progress);
            }
        }

        thread::sleep(Duration::from_secs(1));
    }

    Err(color_eyre::eyre::eyre!(
        "Timeout waiting for SSH connectivity after {}s",
        timeout.as_secs()
    ))
}

/// Run an ephemeral pod and immediately SSH into it, with lifecycle binding
pub fn run_ephemeral_ssh(opts: RunEphemeralSshOpts) -> Result<()> {
    // Start the ephemeral pod in detached mode with SSH enabled
    let mut ephemeral_opts = opts.run_opts.clone();
    ephemeral_opts.podman.rm = true;
    ephemeral_opts.podman.detach = true;
    ephemeral_opts.common.ssh_keygen = true; // Enable SSH key generation and access

    debug!("Starting ephemeral VM...");
    let container_id = run_detached(ephemeral_opts)?;
    debug!("Ephemeral VM started with container ID: {}", container_id);

    // Use the container ID for SSH and cleanup
    let container_name = container_id;
    debug!("Using container ID: {}", container_name);

    let progress_bar = crate::boot_progress::create_boot_progress_bar();
    let progress_bar = wait_for_ssh_ready(&container_name, Duration::from_secs(60), progress_bar)?;
    progress_bar.finish_and_clear();

    // Execute SSH connection directly (no thread needed for this)
    // This allows SSH output to be properly forwarded to stdout/stderr
    debug!("Connecting to SSH with args: {:?}", opts.ssh_args);
    let status = ssh::connect_via_container_with_status(&container_name, opts.ssh_args)?;
    debug!("SSH connection completed");

    let exit_code = status.code().unwrap_or(1);
    debug!("SSH exit code: {}", exit_code);

    // SSH completed, proceed with cleanup

    // Cleanup: stop and remove the container immediately
    debug!("SSH session ended, cleaning up ephemeral pod...");

    let _ = Command::new("podman")
        .args(["rm", "-f", &container_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    // Exit with SSH client's exit code
    std::process::exit(exit_code);
}
