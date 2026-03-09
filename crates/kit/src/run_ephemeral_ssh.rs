use color_eyre::eyre::{eyre, Context as _};
use color_eyre::Result;
use indicatif::ProgressBar;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;
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

/// Wait for VM SSH availability using the supervisor status file
///
/// Monitors /run/supervisor-status.json inside the container for SSH.
/// Returns Ok(true) when systemd indicates ssh is probably ready.
/// Returns Ok(false) if we don't support systemd status notifications.
pub fn wait_for_vm_ssh(
    container_name: &str,
    timeout: Option<Duration>,
    progress: ProgressBar,
) -> Result<(bool, ProgressBar)> {
    let timeout = timeout.unwrap_or(SSH_TIMEOUT);

    debug!(
        "Waiting for VM readiness via supervisor status file (timeout: {}s)...",
        timeout.as_secs()
    );

    // Check if container is still running before attempting exec
    if !is_container_running(container_name)? {
        progress.finish_and_clear();
        show_container_logs(container_name);
        return Err(eyre!("Container exited before SSH became available"));
    }

    // Use the new monitor-status subcommand for efficient inotify-based monitoring
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
    let mut child = match cmd.stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn() {
        Ok(child) => child,
        Err(e) => {
            progress.finish_and_clear();
            show_container_logs(container_name);
            return Err(e).context("Failed to start status monitor");
        }
    };

    let stdout = child.stdout.take().unwrap();
    let reader = std::io::BufReader::new(stdout);

    // Read JSON lines from the monitor
    for line in std::io::BufRead::lines(reader) {
        let line = line.context("Reading monitor output")?;

        let status: SupervisorStatus = serde_json::from_str(&line)
            .with_context(|| format!("Failed to parse monitor output as JSON: {}", line))?;
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

    progress.finish_and_clear();
    show_container_logs(container_name);
    Err(eyre!("Monitor process exited unexpectedly: {status:?}"))
}

/// Wait for SSH to be ready by polling SSH connection attempts
///
/// Attempts to connect to the VM via SSH until successful or timeout.
/// This is used as a fallback when systemd notification is not available.
pub fn wait_for_ssh_ready(
    container_name: &str,
    timeout: Option<Duration>,
    progress: ProgressBar,
) -> Result<(std::time::Duration, ProgressBar)> {
    let timeout = timeout.unwrap_or(SSH_TIMEOUT);
    let (_, progress) = wait_for_vm_ssh(container_name, Some(timeout), progress)?;

    debug!("Polling SSH connectivity...");

    // Use SSH options optimized for connectivity testing
    let ssh_options = crate::ssh::SshConnectionOptions::for_connectivity_test();
    let container_name = container_name.to_string();

    // Use shared polling function with container-specific test
    crate::utils::wait_for_readiness(
        progress,
        "Waiting for SSH",
        || {
            // Try to connect via SSH and run a simple command
            let status = crate::ssh::connect(
                &container_name,
                vec!["true".to_string()], // Just run 'true' to test connectivity
                &ssh_options,
            );

            match status {
                Ok(exit_status) if exit_status.success() => Ok(true),
                _ => Ok(false),
            }
        },
        timeout,
        Duration::from_secs(1), // Poll every 1 second
    )
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
