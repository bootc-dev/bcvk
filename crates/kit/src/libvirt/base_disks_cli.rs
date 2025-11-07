//! Base disk management CLI commands
//!
//! This module provides CLI commands for managing base disk images that serve
//! as CoW sources for VM disks.

use clap::{Parser, Subcommand};
use color_eyre::Result;
use comfy_table::{presets::UTF8_FULL, Table};
use serde_json;

use super::base_disks::{list_base_disks, prune_base_disks};
use super::OutputFormat;

/// Options for base-disks command
#[derive(Debug, Parser)]
pub struct LibvirtBaseDisksOpts {
    #[command(subcommand)]
    pub command: BaseDisksSubcommand,
}

/// Base disk subcommands
#[derive(Debug, Subcommand)]
pub enum BaseDisksSubcommand {
    /// List all base disk images
    List(ListOpts),
    /// Prune unreferenced base disk images
    Prune(PruneOpts),
}

/// Options for list command
#[derive(Debug, Parser)]
pub struct ListOpts {
    /// Output format
    #[clap(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Options for prune command
#[derive(Debug, Parser)]
pub struct PruneOpts {
    /// Show what would be removed without actually removing
    #[clap(long)]
    pub dry_run: bool,
}

/// Execute the base-disks command
pub fn run(global_opts: &crate::libvirt::LibvirtOptions, opts: LibvirtBaseDisksOpts) -> Result<()> {
    let connect_uri = global_opts.connect.as_deref();

    match opts.command {
        BaseDisksSubcommand::List(list_opts) => run_list(connect_uri, list_opts),
        BaseDisksSubcommand::Prune(prune_opts) => run_prune(connect_uri, prune_opts),
    }
}

/// Execute the list subcommand
fn run_list(connect_uri: Option<&str>, opts: ListOpts) -> Result<()> {
    let base_disks = list_base_disks(connect_uri)?;

    match opts.format {
        OutputFormat::Table => {
            if base_disks.is_empty() {
                println!("No base disk images found");
                return Ok(());
            }

            let mut table = Table::new();
            table.load_preset(UTF8_FULL);
            table.set_header(vec!["NAME", "SIZE", "REFS", "CREATED", "IMAGE DIGEST"]);

            for disk in &base_disks {
                let name = disk.path.file_name().unwrap_or("unknown");

                let size = disk
                    .size
                    .map(|bytes| indicatif::BinaryBytes(bytes).to_string())
                    .unwrap_or_else(|| "unknown".to_string());

                let refs = disk.ref_count.to_string();

                let created = disk
                    .created
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0))
                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "unknown".to_string());

                let digest = disk
                    .image_digest
                    .as_ref()
                    .map(|d| {
                        // Truncate long digests for display
                        if d.len() > 56 {
                            format!("{}...", &d[..53])
                        } else {
                            d.clone()
                        }
                    })
                    .unwrap_or_else(|| "<no metadata>".to_string());

                table.add_row(vec![name, &size, &refs, &created, &digest]);
            }

            println!("{}", table);
            println!(
                "\nFound {} base disk{}",
                base_disks.len(),
                if base_disks.len() == 1 { "" } else { "s" }
            );
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&base_disks)?);
        }
        OutputFormat::Yaml => {
            return Err(color_eyre::eyre::eyre!(
                "YAML format is not supported for base-disks list command"
            ))
        }
        OutputFormat::Xml => {
            return Err(color_eyre::eyre::eyre!(
                "XML format is not supported for base-disks list command"
            ))
        }
    }

    Ok(())
}

/// Execute the prune subcommand
fn run_prune(connect_uri: Option<&str>, opts: PruneOpts) -> Result<()> {
    if opts.dry_run {
        println!("Dry run: showing base disks that would be removed");
    }

    let pruned = prune_base_disks(connect_uri, opts.dry_run)?;

    if pruned.is_empty() {
        println!("No unreferenced base disks found to remove");
    } else {
        println!(
            "\n{} {} base disk{}",
            if opts.dry_run {
                "Would remove"
            } else {
                "Removed"
            },
            pruned.len(),
            if pruned.len() == 1 { "" } else { "s" }
        );
    }

    Ok(())
}
