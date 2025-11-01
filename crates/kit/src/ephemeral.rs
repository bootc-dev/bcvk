//! Ephemeral VM management commands
//!
//! This module provides subcommands for running bootc containers as ephemeral virtual machines.
//! Ephemeral VMs are temporary, non-persistent VMs that are useful for testing, development,
//! and CI/CD workflows.

use std::process::Command;

use clap::Subcommand;
use color_eyre::{eyre::eyre, Result};
use comfy_table::{presets::UTF8_FULL, Table};
use serde::{Deserialize, Serialize};

// Re-export the existing implementations
use crate::run_ephemeral;
use crate::run_ephemeral_ssh;
use crate::ssh;

/// Label used to identify bcvk ephemeral containers
const EPHEMERAL_LABEL: &str = "bcvk.ephemeral=1";

/// SSH connection options for accessing running VMs.
///
/// Provides secure shell access to VMs running within containers,
/// with automatic key management and connection routing.
#[derive(clap::Parser, Debug)]
pub struct SshOpts {
    /// Name or ID of the container running the target VM
    ///
    /// This should match the container name from podman or the VM ID
    /// used when starting the ephemeral VM.
    pub container_name: String,

    /// Additional SSH client arguments to pass through
    ///
    /// Standard ssh arguments like -v for verbose output, -L for
    /// port forwarding, or -o for SSH options.
    #[clap(allow_hyphen_values = true, help = "SSH arguments like -v, -L, -o")]
    pub args: Vec<String>,
}

/// Container list entry for ephemeral VMs
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerListEntry {
    /// Container ID
    pub id: String,

    /// Container names
    pub names: Vec<String>,

    /// Container state
    pub state: String,

    /// Creation timestamp
    pub created_at: String,

    /// Container image
    pub image: String,

    /// Container command
    pub command: Vec<String>,
}

/// Ephemeral VM operations
#[derive(Debug, Subcommand)]
pub enum EphemeralCommands {
    /// Run bootc containers as ephemeral VMs
    #[clap(name = "run")]
    Run(run_ephemeral::RunEphemeralOpts),

    /// Run ephemeral VM and SSH into it
    #[clap(name = "run-ssh")]
    RunSsh(run_ephemeral_ssh::RunEphemeralSshOpts),

    /// Connect to running VMs via SSH
    #[clap(name = "ssh")]
    Ssh(SshOpts),

    /// List ephemeral VM containers
    #[clap(name = "ps")]
    Ps {
        /// Output as structured JSON instead of table format
        #[clap(long)]
        json: bool,
    },

    /// Remove all ephemeral VM containers
    #[clap(name = "rm-all")]
    RmAll {
        /// Force removal without confirmation
        #[clap(short, long)]
        force: bool,
    },
}

impl EphemeralCommands {
    /// Execute the ephemeral subcommand
    pub fn run(self) -> Result<()> {
        match self {
            EphemeralCommands::Run(opts) => run_ephemeral::run(opts),
            EphemeralCommands::RunSsh(opts) => run_ephemeral_ssh::run_ephemeral_ssh(opts),
            EphemeralCommands::Ssh(opts) => {
                // Create progress bar if stderr is a terminal
                let progress_bar = crate::boot_progress::create_boot_progress_bar();

                run_ephemeral_ssh::wait_for_ssh_ready(&opts.container_name, None, progress_bar)?;

                ssh::connect_via_container(&opts.container_name, opts.args)
            }
            EphemeralCommands::Ps { json } => {
                let containers = list_ephemeral_containers()?;

                if json {
                    let json_output = serde_json::to_string_pretty(&containers)?;
                    println!("{}", json_output);
                } else {
                    // Create a table using comfy_table
                    let mut table = Table::new();
                    table.load_preset(UTF8_FULL).set_header(vec![
                        "CONTAINER ID",
                        "IMAGE",
                        "CREATED",
                        "STATUS",
                        "NAMES",
                    ]);

                    for container in containers {
                        let id = if container.id.len() > 12 {
                            &container.id[..12]
                        } else {
                            &container.id
                        };

                        let names = container.names.join(", ");
                        let image = if container.image.len() > 30 {
                            format!("{}...", &container.image[..30])
                        } else {
                            container.image.clone()
                        };

                        table.add_row(vec![
                            id.to_string(),
                            image,
                            container.created_at,
                            container.state,
                            names,
                        ]);
                    }

                    println!("{}", table);
                }
                Ok(())
            }
            EphemeralCommands::RmAll { force } => remove_all_ephemeral_containers(force),
        }
    }
}

/// List ephemeral VM containers with bcvk.ephemeral=1 label
fn list_ephemeral_containers() -> Result<Vec<ContainerListEntry>> {
    use bootc_utils::CommandRunExt;

    let containers: Vec<ContainerListEntry> = Command::new("podman")
        .args([
            "ps",
            "--all",
            "--format",
            "json",
            &format!("--filter=label={}", EPHEMERAL_LABEL),
        ])
        .run_and_parse_json()
        .map_err(|e| eyre!("Failed to list ephemeral containers: {}", e))?;
    Ok(containers)
}

/// Remove all ephemeral VM containers
fn remove_all_ephemeral_containers(force: bool) -> Result<()> {
    use bootc_utils::CommandRunExt;

    let containers = list_ephemeral_containers()?;

    if containers.is_empty() {
        println!("No ephemeral containers found.");
        return Ok(());
    }

    if !force {
        println!("Found {} ephemeral container(s):", containers.len());
        for container in &containers {
            let id = if container.id.len() > 12 {
                &container.id[..12]
            } else {
                &container.id
            };
            let names = container.names.join(", ");
            println!("  {} ({})", id, names);
        }

        print!("Remove all ephemeral containers? [y/N]: ");
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            println!("Aborted.");
            return Ok(());
        }
    }

    for container in &containers {
        println!(
            "Removing container {}",
            &container.id[..12.min(container.id.len())]
        );
        let result = Command::new("podman")
            .args(["rm", "-f", &container.id])
            .run();

        match result {
            Ok(_) => println!("Removed {}", &container.id[..12.min(container.id.len())]),
            Err(e) => eprintln!(
                "Failed to remove {}: {}",
                &container.id[..12.min(container.id.len())],
                e
            ),
        }
    }

    Ok(())
}
