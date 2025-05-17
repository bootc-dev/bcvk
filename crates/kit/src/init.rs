//! Implementation of the `init` command for setting up bootc-kit infrastructure
//!
//! This initializes core infrastructure, like setting up cstor-dist and
//! configuring shell aliases for easier access.

use std::fs::{create_dir_all, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use bootc_utils::CommandRunExt;
use color_eyre::{eyre::eyre, Result};
use tracing::instrument;

use crate::hostexec;

/// Name of the alias script we'll offer to create
const ALIAS_SCRIPT_NAME: &str = "bck";
/// Default location for the alias script
const DEFAULT_ALIAS_PATH: &str = ".local/bin";

/// Options for the init command
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct InitOpts {
    /// Skip the prompt to set up shell alias
    #[clap(long)]
    skip_alias_prompt: bool,
    
    /// Path where to write the shell alias (default: ~/.local/bin/bck)
    #[clap(long)]
    alias_path: Option<String>,
}

impl InitOpts {
    #[instrument]
    pub(crate) fn run(&self) -> Result<()> {
        // Set up cstor-dist
        setup_cstor_dist()?;
        
        // Set up alias if requested
        if !self.skip_alias_prompt {
            setup_shell_alias(self.alias_path.as_deref())?;
        }
        
        println!("Initialization complete!");
        Ok(())
    }
}

/// Set up an instance of cstor-dist for the user
#[instrument]
fn setup_cstor_dist() -> Result<()> {
    println!("Setting up cstor-dist...");
    
    // Check if it's already running
    let status = hostexec::command("podman", None)?
        .args(["ps", "--filter", "name=cstor-dist", "--format", "{{.Names}}"])
        .output()
        .map_err(|e| eyre!("Failed to check if cstor-dist is running: {}", e))?;
    
    if !String::from_utf8_lossy(&status.stdout).trim().is_empty() {
        println!("cstor-dist is already running");
        return Ok(());
    }
    
    // Start cstor-dist
    println!("Starting cstor-dist container...");
    hostexec::command("podman", None)?
        .args([
            "run", 
            "-d", 
            "--name", "cstor-dist",
            "--restart", "always",
            "ghcr.io/cgwalters/cstor-dist:latest"
        ])
        .run()
        .map_err(|e| eyre!("Failed to start cstor-dist container: {}", e))?;
    
    println!("cstor-dist has been set up successfully");
    Ok(())
}

/// Set up a shell alias for the user to easily access bootc-kit
#[instrument]
fn setup_shell_alias(custom_path: Option<&str>) -> Result<()> {
    let home = std::env::var("HOME").map_err(|e| eyre!("Querying $HOME: {}", e))?;
    
    println!("Setting up shell alias for bootc-kit...");
    println!("This will create a script that makes it easier to use bootc-kit.");
    
    // Ask the user if they want to set up the alias
    println!("Would you like to set up an alias for bootc-kit? [Y/n]");
    let mut input = String::new();
    io::stdin().read_line(&mut input).map_err(|e| eyre!("Failed to read user input: {}", e))?;
    
    if input.trim().to_lowercase() == "n" {
        println!("Skipping alias setup");
        return Ok(());
    }
    
    // Determine the path where to write the alias
    let alias_dir = if let Some(path) = custom_path {
        Path::new(path).to_path_buf()
    } else {
        let default_path = format!("{}/{}", home, DEFAULT_ALIAS_PATH);
        println!("Where would you like to install the alias? [{}]", default_path);
        let mut path_input = String::new();
        io::stdin().read_line(&mut path_input).map_err(|e| eyre!("Failed to read user input: {}", e))?;
        
        if path_input.trim().is_empty() {
            Path::new(&default_path).to_path_buf()
        } else {
            Path::new(path_input.trim()).to_path_buf()
        }
    };
    
    // Make sure the directory exists
    if !alias_dir.exists() {
        create_dir_all(&alias_dir).map_err(|e| eyre!("Failed to create directory for alias: {}", e))?;
    }
    
    // Build the full path for the script
    let script_path = alias_dir.join(ALIAS_SCRIPT_NAME);
    
    // Check if the file already exists
    if script_path.exists() {
        println!("The file {} already exists. Overwrite? [y/N]", script_path.display());
        let mut overwrite_input = String::new();
        io::stdin().read_line(&mut overwrite_input).map_err(|e| eyre!("Failed to read user input: {}", e))?;
        
        if overwrite_input.trim().to_lowercase() != "y" {
            println!("Skipping alias creation");
            return Ok(());
        }
    }
    
    // Create the script file
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&script_path)
        .map_err(|e| eyre!("Failed to create alias script: {}", e))?;
    
    // Set the file permissions to executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script_path).map_err(|e| eyre!("Failed to get file permissions: {}", e))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms)?;
    }
    
    // Write the script content
    let script_content = r#"#!/bin/bash
set -euo pipefail
# Set of args for podman. We always kill the container on
# exit, and pass stdin.
args=(--rm -i)
# If stdin is a terminal, then tell podman to make one too.
if [ -t 0 ]; then
    args+=(-t)
fi
# Allow overriding the image.
BOOTC_KIT_IMAGE=${BOOTC_KIT_IMAGE:-ghcr.io/bootc-dev/kit}
# Isolation/security options. In the general case we need to spawn
# things on the host.
args+=(--net=host --privileged --pid=host)
# However by default keep the image read only, just on general principle.
args+=(--read-only --read-only-tmpfs)
# Default to passing through the current working directory.
args+=(-v $(pwd):/run/context -w /run/context)
# And spawn the container.
exec podman run ${args[@]} "${BOOTC_KIT_IMAGE}" bootc-kit "$@"
"#;
    
    file.write_all(script_content.as_bytes()).map_err(|e| eyre!("Failed to write alias script: {}", e))?;
    
    println!("Alias script created at: {}", script_path.display());
    println!("Make sure {} is in your PATH", alias_dir.display());
    
    // Check if the directory is in PATH
    let path_var = std::env::var("PATH").unwrap_or_default();
    if !path_var.split(':').any(|p| p == alias_dir.to_string_lossy()) {
        println!("WARNING: {} is not in your PATH", alias_dir.display());
        println!("You may want to add it to your shell configuration:");
        println!("  export PATH=\"$PATH:{}\"", alias_dir.display());
    }
    
    Ok(())
}