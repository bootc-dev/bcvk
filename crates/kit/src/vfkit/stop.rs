//! vm stop — Stop a running persistent VM.

use std::time::Duration;

use super::VmMetadata;
use color_eyre::{eyre::bail, Result};
use tracing::info;

/// Options for `vm stop`.
#[derive(clap::Parser, Debug)]
pub struct VmStopOpts {
    /// VM name
    pub name: String,
    /// Force immediate power-off (SIGKILL) instead of graceful shutdown
    #[clap(long, short = 'f')]
    pub force: bool,
}

/// Stop a running persistent VM.
pub fn run(opts: VmStopOpts) -> Result<()> {
    let mut meta = VmMetadata::load(&opts.name)?;
    if !meta.is_alive() {
        bail!("VM '{}' is not running", opts.name);
    }

    info!("stopping VM '{}'...", opts.name);

    if meta.vfkit_pid > 0 {
        let pid = rustix::process::Pid::from_raw(meta.vfkit_pid as i32).unwrap();
        if opts.force {
            if let Err(e) = rustix::process::kill_process(pid, rustix::process::Signal::KILL) {
                tracing::debug!("failed to SIGKILL vfkit (PID {}): {}", meta.vfkit_pid, e);
            }
        } else {
            if let Err(e) = rustix::process::kill_process(pid, rustix::process::Signal::TERM) {
                tracing::debug!("failed to SIGTERM vfkit (PID {}): {}", meta.vfkit_pid, e);
            }
            std::thread::sleep(Duration::from_secs(3));
            if meta.is_alive() {
                if let Err(e) = rustix::process::kill_process(pid, rustix::process::Signal::KILL) {
                    tracing::debug!("failed to SIGKILL vfkit (PID {}): {}", meta.vfkit_pid, e);
                }
            }
        }
    }

    if meta.gvproxy_pid > 0 {
        if let Err(e) = rustix::process::kill_process(
            rustix::process::Pid::from_raw(meta.gvproxy_pid as i32).unwrap(),
            rustix::process::Signal::KILL,
        ) {
            tracing::debug!(
                "failed to SIGKILL gvproxy (PID {}): {}",
                meta.gvproxy_pid,
                e
            );
        }
    }

    meta.state = "stopped".to_string();
    meta.vfkit_pid = 0;
    meta.gvproxy_pid = 0;
    meta.save()?;

    println!("Stopped '{}'", opts.name);
    Ok(())
}
