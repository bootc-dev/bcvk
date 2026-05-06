//! vm stop — Stop a running persistent VM.

use std::process::{Command, Stdio};
use std::time::Duration;

use super::VmMetadata;
use color_eyre::{eyre::bail, Result};
use tracing::info;

/// Stop a running persistent VM by sending SIGTERM to vfkit.
pub fn run(name: &str) -> Result<()> {
    let mut meta = VmMetadata::load(name)?;
    if !meta.is_alive() {
        bail!("VM '{}' is not running", name);
    }

    info!("stopping VM '{}'...", name);

    if meta.vfkit_pid > 0 {
        if let Err(e) = Command::new("kill")
            .args(["-TERM", &meta.vfkit_pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            tracing::debug!("failed to SIGTERM vfkit (PID {}): {}", meta.vfkit_pid, e);
        }
        std::thread::sleep(Duration::from_secs(3));
        if meta.is_alive() {
            if let Err(e) = Command::new("kill")
                .args(["-KILL", &meta.vfkit_pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
            {
                tracing::debug!("failed to SIGKILL vfkit (PID {}): {}", meta.vfkit_pid, e);
            }
        }
    }

    if meta.gvproxy_pid > 0 {
        if let Err(e) = Command::new("kill")
            .args(["-KILL", &meta.gvproxy_pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
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

    println!("Stopped '{}'", name);
    Ok(())
}
