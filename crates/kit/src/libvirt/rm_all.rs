//! libvirt rm-all command - remove multiple bootc domains and their resources
//!
//! This module provides functionality to remove multiple libvirt domains
//! and their associated resources at once, with optional label filtering.

use clap::Parser;
use color_eyre::Result;

/// Options for removing multiple libvirt domains
#[derive(Debug, Parser)]
pub struct LibvirtRmAllOpts {
    /// Force removal without confirmation
    #[clap(long, short = 'f')]
    pub force: bool,

    /// Remove domains even if they're running
    #[clap(long)]
    pub stop: bool,

    /// Filter domains by label (only remove domains with this label)
    #[clap(long)]
    pub label: Option<String>,
}

/// Execute the libvirt rm-all command
pub fn run(global_opts: &crate::libvirt::LibvirtOptions, opts: LibvirtRmAllOpts) -> Result<()> {
    use crate::domain_list::DomainLister;
    use color_eyre::eyre::Context;

    let connect_uri = global_opts.connect.as_ref();
    let lister = match connect_uri {
        Some(uri) => DomainLister::with_connection(uri.clone()),
        None => DomainLister::new(),
    };

    // Get all bootc domains
    let mut domains = lister
        .list_bootc_domains()
        .with_context(|| "Failed to list bootc domains from libvirt")?;

    // Filter by label if specified
    if let Some(ref filter_label) = opts.label {
        domains.retain(|d| d.labels.contains(filter_label));
    }

    if domains.is_empty() {
        if let Some(ref label) = opts.label {
            println!("No VMs found with label '{}'", label);
        } else {
            println!("No VMs found");
        }
        return Ok(());
    }

    // Confirmation prompt
    if !opts.force {
        println!(
            "This will permanently delete {} VM{} and their data:",
            domains.len(),
            if domains.len() == 1 { "" } else { "s" }
        );
        for domain in &domains {
            println!("  - {} ({})", domain.name, domain.status_string());
            if let Some(ref image) = domain.image {
                println!("    Image: {}", image);
            }
            if let Some(ref disk_path) = domain.disk_path {
                println!("    Disk: {}", disk_path);
            }
            if !domain.labels.is_empty() {
                println!("    Labels: {}", domain.labels.join(", "));
            }
        }
        println!();
        println!("Are you sure? This cannot be undone. Use --force to skip this prompt.");
        return Ok(());
    }

    let mut removed_count = 0;
    let mut error_count = 0;

    for domain in &domains {
        println!("Removing VM '{}'...", domain.name);
        let owned_files = crate::libvirt::rm::domain_owned_files(global_opts, &domain.name);

        // Stop if running
        if domain.is_running() {
            if opts.stop {
                println!("  Stopping running VM...");
                let output = global_opts
                    .virsh_command()
                    .args(&["destroy", &domain.name])
                    .output()
                    .with_context(|| format!("Failed to stop VM '{}'", domain.name))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    eprintln!("  Failed to stop VM '{}': {}", domain.name, stderr);
                    error_count += 1;
                    continue;
                }
            } else {
                eprintln!(
                    "  Skipping '{}': VM is running. Use --stop to force removal.",
                    domain.name
                );
                error_count += 1;
                continue;
            }
        }

        // Remove disk manually if it exists (unmanaged storage)
        if let Some(ref disk_path) = domain.disk_path {
            if std::path::Path::new(disk_path).exists() {
                println!("  Removing disk image...");
                if let Err(e) = std::fs::remove_file(disk_path) {
                    eprintln!(
                        "  Warning: Failed to remove disk file '{}': {}",
                        disk_path, e
                    );
                    // Continue anyway - libvirt may still have the domain
                }
            }
        }

        // Remove libvirt domain with nvram
        println!("  Removing libvirt domain...");
        let output = global_opts
            .virsh_command()
            .args(&["undefine", &domain.name, "--nvram"])
            .output()
            .with_context(|| format!("Failed to undefine domain '{}'", domain.name))?;

        if output.status.success() {
            crate::libvirt::rm::remove_files(&owned_files);
            println!("  VM '{}' removed successfully", domain.name);
            removed_count += 1;
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!(
                "  Failed to remove libvirt domain '{}': {}",
                domain.name, stderr
            );
            error_count += 1;
        }
    }

    println!();
    println!(
        "Summary: {} VM{} removed, {} error{}",
        removed_count,
        if removed_count == 1 { "" } else { "s" },
        error_count,
        if error_count == 1 { "" } else { "s" }
    );

    if error_count > 0 {
        Err(color_eyre::eyre::eyre!(
            "Failed to remove {} VM{}",
            error_count,
            if error_count == 1 { "" } else { "s" }
        ))
    } else {
        Ok(())
    }
}
