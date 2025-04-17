//! Utilities for generating entrypoint scripts
//!
//! This module provides functionality to generate entrypoint scripts
//! for different platforms.

use color_eyre::Result;
use tracing::instrument;

/// Print the entrypoint script for the current platform
#[instrument]
pub fn print_entrypoint_script() -> Result<()> {
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            println!("{}", include_str!("entrypoint-macos.sh"));
        } else if #[cfg(target_os = "linux")] {
            println!("{}", include_str!("entrypoint-linux.sh"));
        } else {
            return Err(eyre!("Unsupported platform"));
        }
    };
    Ok(())
}
