//! Implementation of the `init` command for setting up bootc-kit infrastructure
//!
//! This initializes core infrastructure, like setting up cstor-dist and
//! configuring shell aliases for easier access.

use bootc_utils::CommandRunExt;
use color_eyre::{eyre::eyre, Result};
use tracing::instrument;

use crate::hostexec;

/// Default cstor-dist image
const DEFAULT_CSTOR_DIST_IMAGE: &str = "ghcr.io/cgwalters/cstor-dist:latest";
/// Environment variable to override the cstor-dist image
const CSTOR_DIST_IMAGE_ENV: &str = "CSTOR_DIST_IMAGE";
/// Default TCP port to listen on for cstor-dist
const DEFAULT_CSTOR_DIST_PORT: u16 = 9050;
/// Environment variable to override the cstor-dist port
const CSTOR_DIST_PORT_ENV: &str = "CSTOR_DIST_PORT";

/// Options for the init command
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct InitOpts {}

impl InitOpts {
    #[instrument]
    pub(crate) fn run(&self) -> Result<()> {
        // Set up cstor-dist
        setup_cstor_dist()?;

        println!("Initialization complete!");
        Ok(())
    }
}

/// Set up an instance of cstor-dist for the user
#[instrument]
fn setup_cstor_dist() -> Result<()> {
    println!("Setting up cstor-dist...");

    // Check if it's already running
    let status = hostexec::podman()?
        .args([
            "ps",
            "--filter",
            "name=cstor-dist",
            "--format",
            "{{.Names}}",
        ])
        .output()
        .map_err(|e| eyre!("Failed to check if cstor-dist is running: {}", e))?;

    if !String::from_utf8_lossy(&status.stdout).trim().is_empty() {
        println!("cstor-dist is already running");
        return Ok(());
    }

    let cstor_dist_image = std::env::var(CSTOR_DIST_IMAGE_ENV)
        .unwrap_or_else(|_| DEFAULT_CSTOR_DIST_IMAGE.to_string());
    let port = std::env::var_os(CSTOR_DIST_PORT_ENV);
    let port = port
        .as_ref()
        .and_then(|p| p.to_str())
        .map(|p| p.parse::<u16>().map_err(|e| eyre!("Invalid port: {}", e)))
        .transpose()?
        .unwrap_or(DEFAULT_CSTOR_DIST_PORT);

    // Start cstor-dist
    println!(
        "Starting cstor-dist container using image: {}...",
        cstor_dist_image
    );
    hostexec::podman()?
        .args(["run", "-d", "--name", "cstor-dist"])
        .arg(format!("--publish={port}:8000"))
        .arg(cstor_dist_image.as_str())
        .run()
        .map_err(|e| eyre!("Failed to start cstor-dist container: {}", e))?;

    println!("cstor-dist has been set up successfully");
    Ok(())
}
