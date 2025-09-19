use color_eyre::Result;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::{fs::File, io::BufRead, time::Duration};

use crate::supervisor_status::{StatusWriter, SupervisorState, SupervisorStatus};

/// Create a progress bar for boot status
pub fn create_boot_progress_bar() -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_draw_target(ProgressDrawTarget::stderr());
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_message("Starting VM...");
    pb
}

/// Monitor systemd boot progress and update progress bar
pub async fn monitor_boot_progress(piper: File, status_writer: StatusWriter) -> Result<()> {
    // Update status to indicate we're waiting for systemd
    status_writer.update_state(SupervisorState::WaitingForSystemd)?;

    let bufr = std::io::BufReader::new(piper);

    for line in bufr.lines() {
        let line = line?;
        let line = line.trim();

        let Some((k, v)) = line.split_once('=') else {
            tracing::trace!("Unhandled status line: {line}");
            continue;
        };
        match k {
            "READY" => {
                status_writer.update(SupervisorStatus::new(SupervisorState::Ready))?;
            }
            "X_SYSTEMD_UNIT_ACTIVE" => {
                status_writer.update_state(SupervisorState::ReachedTarget(v.to_owned()))?;
            }
            _ => {
                tracing::trace!("Unhandled status line: {line}")
            }
        }
    }

    Ok(())
}
