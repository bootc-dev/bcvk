//! Process lifecycle monitor for parent process binding
//!
//! This module implements background monitoring of a parent process and executing a command
//! when the parent exits. Used by the `--lifecycle-bind-parent` flag to automatically shut
//! down VMs when the parent `bcvk` process exits.
//!
//! # Architecture
//!
//! ## pidfd-Based Monitoring with Fallback
//!
//! The monitor uses modern Linux kernel features for efficient parent process monitoring:
//!
//! 1. **pidfd + poll()** - Opens a pidfd for the parent process and blocks on `poll()` waiting
//!    for it to become readable (process exit)
//! 2. **Fallback to /proc polling** - If `pidfd_open()` fails (ENOSYS/EPERM), falls back to
//!    polling `/proc/<pid>/` every 1 second
//! 3. **Signal Handlers** - Tokio signal handlers for SIGTERM and SIGINT
//! 4. **Event Loop** - `tokio::select!` waits for any of these events
//!
//! ## Key Design Decisions
//!
//! **pidfd over set_parent_process_death_signal:**
//! - `set_parent_process_death_signal` doesn't work when parent exits immediately
//! - pidfd allows monitoring an arbitrary PID, not just direct parent
//! - Efficient: `poll()` blocks until process exits (no busy polling)
//!
//! **Graceful Fallback:**
//! - pidfd requires Linux kernel 5.3+ (2019)
//! - Falls back to `/proc` polling if unsupported
//! - Handles ENOSYS (kernel too old) and EPERM (permission denied)
//!
//! **Generic Command Execution:**
//! - Accepts arbitrary command to run when parent exits
//! - Makes it testable with simple commands like `echo "test"`
//! - Generalizable for any cleanup action, not just VM shutdown
//!
//! # Usage
//!
//! ## Automatic (Project VMs)
//!
//! ```bash
//! bcvk project up  # Lifecycle binding enabled by default
//! ```
//!
//! ## Explicit
//!
//! ```bash
//! bcvk libvirt run --lifecycle-bind-parent --name my-vm quay.io/fedora/fedora-bootc:42
//! ```
//!
//! ## Direct (Testing/Debugging)
//!
//! ```bash
//! # Monitor a specific PID and run command when it exits
//! bcvk internals lifecycle-monitor 12345 virsh shutdown my-vm
//!
//! # Use "parent" to monitor the actual parent process
//! bcvk internals lifecycle-monitor parent echo "Parent exited"
//!
//! # With libvirt connection URI
//! bcvk internals lifecycle-monitor 12345 virsh -c qemu:///session shutdown my-vm
//! ```
//!
//! # Testing
//!
//! ```bash
//! # Test with a temporary process
//! sleep 10 &
//! TEST_PID=$!
//! bcvk internals lifecycle-monitor $TEST_PID echo "Sleep process exited"
//! # The monitor will print the message when sleep exits after 10 seconds
//! ```

use clap::Parser;
use color_eyre::{eyre::Context, Result};
use rustix::event::{poll, PollFd, PollFlags};
use rustix::fd::OwnedFd;
use rustix::process::{getppid, pidfd_open, PidfdFlags};
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tokio::signal::unix::{signal, SignalKind};
use tracing::debug;

/// Internal command to monitor parent process and execute command on exit
#[derive(Debug, Parser)]
pub struct LifecycleMonitorOpts {
    /// Parent process ID to monitor (numeric PID or "parent" for actual parent process)
    pub parent_pid: String,

    /// Command and arguments to run when parent exits
    #[clap(trailing_var_arg = true, required = true)]
    pub command: Vec<String>,
}

/// Resolve the parent PID string to a numeric PID
fn resolve_parent_pid(pid_str: &str) -> Result<u32> {
    if pid_str == "parent" {
        // Get the actual parent process ID using rustix
        let ppid =
            getppid().ok_or_else(|| color_eyre::eyre::eyre!("Failed to get parent process ID"))?;
        Ok(ppid.as_raw_nonzero().get() as u32)
    } else {
        // Parse as numeric PID
        pid_str.parse::<u32>().with_context(|| {
            format!(
                "Invalid PID: '{}' (expected numeric PID or 'parent')",
                pid_str
            )
        })
    }
}

/// Execute the lifecycle monitor
#[allow(unsafe_code)]
pub async fn run_async(opts: LifecycleMonitorOpts) -> Result<()> {
    let parent_pid = resolve_parent_pid(&opts.parent_pid)?;
    let command = opts.command.clone();

    debug!(
        "Starting lifecycle monitor for parent PID {} (command: {:?})",
        parent_pid, command
    );

    // Try to open pidfd for the parent process, fall back to polling if unsupported
    let wait_method = match open_pidfd(parent_pid) {
        Ok(pidfd) => {
            debug!("Using pidfd for parent process monitoring");
            WaitMethod::Pidfd(pidfd)
        }
        Err(e) => {
            debug!("pidfd_open failed ({}), falling back to /proc polling", e);
            WaitMethod::ProcPolling
        }
    };

    // Set up signal handlers for SIGTERM and SIGINT
    let mut sigterm =
        signal(SignalKind::terminate()).context("Failed to create SIGTERM handler")?;
    let mut sigint = signal(SignalKind::interrupt()).context("Failed to create SIGINT handler")?;

    debug!(
        "Monitoring parent process {} and waiting for shutdown signals (SIGTERM, SIGINT)",
        parent_pid
    );

    // Spawn blocking task to wait for parent exit
    let wait_task =
        tokio::task::spawn_blocking(move || wait_for_parent_exit(wait_method, parent_pid));

    // Wait for either parent exit or signals
    tokio::select! {
        result = wait_task => {
            match result {
                Ok(Ok(())) => debug!("Parent process {} exited", parent_pid),
                Ok(Err(e)) => debug!("Error waiting for parent process: {}", e),
                Err(e) => debug!("Wait task panicked: {}", e),
            }
        }
        _ = sigterm.recv() => {
            debug!("SIGTERM received");
        }
        _ = sigint.recv() => {
            debug!("SIGINT received");
        }
    }

    debug!(
        "Shutdown trigger received, executing command: {:?}",
        command
    );

    // Execute the command
    if let Err(e) = execute_command(&command) {
        debug!("Failed to execute command: {}", e);
        std::process::exit(1);
    }

    // Exit the process immediately without waiting for tokio runtime shutdown.
    // This is important because the spawned blocking task may still be running
    // (blocking on poll), and rt.block_on() won't return until all tasks complete.
    // Using process::exit() bypasses the runtime shutdown and exits immediately.
    std::process::exit(0);
}

/// Method for waiting on parent process exit
enum WaitMethod {
    /// Use pidfd (modern, efficient)
    Pidfd(OwnedFd),
    /// Use /proc polling (fallback)
    ProcPolling,
}

/// Try to open pidfd for the parent process, handling errors for unsupported systems
fn open_pidfd(pid: u32) -> Result<OwnedFd> {
    let pid_raw = rustix::process::Pid::from_raw(pid as i32)
        .ok_or_else(|| color_eyre::eyre::eyre!("Invalid PID: {}", pid))?;

    match pidfd_open(pid_raw, PidfdFlags::empty()) {
        Ok(fd) => Ok(fd),
        Err(rustix::io::Errno::NOSYS) => {
            Err(color_eyre::eyre::eyre!("pidfd_open not supported (ENOSYS)"))
        }
        Err(rustix::io::Errno::PERM) => Err(color_eyre::eyre::eyre!(
            "pidfd_open permission denied (EPERM)"
        )),
        Err(e) => Err(color_eyre::eyre::eyre!("pidfd_open failed: {}", e)),
    }
}

/// Wait for parent process to exit using the specified method
fn wait_for_parent_exit(method: WaitMethod, pid: u32) -> Result<()> {
    match method {
        WaitMethod::Pidfd(pidfd) => wait_for_pidfd(pidfd),
        WaitMethod::ProcPolling => wait_for_proc_exit(pid),
    }
}

/// Wait for pidfd to become readable (parent process exit) using poll()
fn wait_for_pidfd(pidfd: OwnedFd) -> Result<()> {
    let mut poll_fds = [PollFd::new(&pidfd, PollFlags::IN)];

    // Block until pidfd is readable (process exits) or error
    // Pass None for infinite timeout
    loop {
        match poll(&mut poll_fds, None) {
            Ok(_) => {
                // Check if POLLIN is set (process exited)
                let revents = poll_fds[0].revents();
                if revents.contains(PollFlags::IN) {
                    debug!("Pidfd became readable - parent process exited");
                    return Ok(());
                }
                // If other event, continue polling
                debug!("Poll returned with revents: {:?}", revents);
            }
            Err(rustix::io::Errno::INTR) => {
                // Interrupted by signal, continue
                debug!("Poll interrupted by signal, continuing");
                continue;
            }
            Err(e) => {
                return Err(color_eyre::eyre::eyre!("poll() failed: {}", e));
            }
        }
    }
}

/// Wait for parent process to exit by polling /proc (fallback)
fn wait_for_proc_exit(pid: u32) -> Result<()> {
    let proc_path = format!("/proc/{}", pid);

    loop {
        if !Path::new(&proc_path).exists() {
            debug!("Process {} no longer exists in /proc", pid);
            return Ok(());
        }

        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Synchronous wrapper for async run
pub fn run(opts: LifecycleMonitorOpts) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Failed to create tokio runtime")?;

    rt.block_on(run_async(opts))
}

/// Execute the configured command without waiting for completion
fn execute_command(command: &[String]) -> Result<()> {
    if command.is_empty() {
        return Err(color_eyre::eyre::eyre!("No command specified"));
    }

    debug!("Executing command: {:?}", command);

    let mut cmd = Command::new(&command[0]);
    if command.len() > 1 {
        cmd.args(&command[1..]);
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to execute command: {:?}", command))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!(
            "Command {:?} failed with status {}: {}",
            command, output.status, stderr
        );
    } else {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.is_empty() {
            debug!("Command output: {}", stdout);
        }
        debug!("Command executed successfully: {:?}", command);
    }

    Ok(())
}
