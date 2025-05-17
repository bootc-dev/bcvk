//! Utilities for generating entrypoint scripts
//!
//! This module provides functionality to generate entrypoint scripts
//! for different platforms.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use color_eyre::{eyre::eyre, Result};
use tracing::instrument;
use include_str;

/// The Linux entrypoint script
const LINUX_ENTRYPOINT: &str = include_str!("entrypoint-linux.sh");

/// The macOS entrypoint script (when available)
#[cfg(target_os = "macos")]
const MACOS_ENTRYPOINT: &str = include_str!("entrypoint-macos.sh");

/// Generate an entrypoint script for the current platform
#[instrument]
pub fn generate_entrypoint_script(path: &Path) -> Result<()> {
    println!("Generating entrypoint script at {}", path.display());
    
    let entrypoint = if cfg!(target_os = "macos") {
        #[cfg(target_os = "macos")]
        {
            MACOS_ENTRYPOINT
        }
        #[cfg(not(target_os = "macos"))]
        {
            return Err(eyre!("macOS entrypoint requested but not available on this platform"));
        }
    } else {
        LINUX_ENTRYPOINT
    };
    
    let mut file = File::create(path)?;
    file.write_all(entrypoint.as_bytes())?;
    
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)?;
    }
    
    println!("Entrypoint script generated successfully");
    Ok(())
}

/// Print the entrypoint script for the current platform
#[instrument]
pub fn print_entrypoint_script() -> Result<()> {
    let entrypoint = if cfg!(target_os = "macos") {
        #[cfg(target_os = "macos")]
        {
            MACOS_ENTRYPOINT
        }
        #[cfg(not(target_os = "macos"))]
        {
            return Err(eyre!("macOS entrypoint requested but not available on this platform"));
        }
    } else {
        LINUX_ENTRYPOINT
    };
    
    println!("{}", entrypoint);
    Ok(())
}