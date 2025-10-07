//! libvirt rm command - remove a bootc domain and its resources
//!
//! This module provides functionality to permanently remove libvirt domains
//! and their associated disk images that were created from bootc container images.

use clap::Parser;
use color_eyre::Result;

/// Options for removing a libvirt domain
#[derive(Debug, Parser)]
pub struct LibvirtRmOpts {
    /// Name of the domain to remove
    pub name: String,

    /// Force removal without confirmation
    #[clap(long, short = 'f')]
    pub force: bool,

    /// Remove domain even if it's running
    #[clap(long)]
    pub stop: bool,
}

/// Execute the libvirt rm command
pub fn run(global_opts: &crate::libvirt::LibvirtOptions, opts: LibvirtRmOpts) -> Result<()> {
    use crate::domain_list::DomainLister;
    use color_eyre::eyre::Context;

    let connect_uri = global_opts.connect.as_ref();
    let lister = match connect_uri {
        Some(uri) => DomainLister::with_connection(uri.clone()),
        None => DomainLister::new(),
    };

    // Check if domain exists and get its state
    let state = lister
        .get_domain_state(&opts.name)
        .map_err(|_| color_eyre::eyre::eyre!("VM '{}' not found", opts.name))?;

    // Get domain info for display
    let domain_info = lister
        .get_domain_info(&opts.name)
        .with_context(|| format!("Failed to get info for VM '{}'", opts.name))?;

    // Check if VM is running
    if state == "running" {
        if opts.stop {
            println!("🛑 Stopping running VM '{}'...", opts.name);
            let output = global_opts
                .virsh_command()
                .args(&["destroy", &opts.name])
                .output()
                .with_context(|| "Failed to stop VM before removal")?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(color_eyre::eyre::eyre!(
                    "Failed to stop VM '{}' before removal: {}",
                    opts.name,
                    stderr
                ));
            }
        } else {
            return Err(color_eyre::eyre::eyre!(
                "VM '{}' is running. Use --stop to force removal or stop it first.",
                opts.name
            ));
        }
    }

    // Confirmation prompt
    if !opts.force {
        println!(
            "This will permanently delete VM '{}' and its data:",
            opts.name
        );
        if let Some(ref image) = domain_info.image {
            println!("  Image: {}", image);
        }
        if let Some(ref disk_path) = domain_info.disk_path {
            println!("  Disk: {}", disk_path);
        }
        println!("  Status: {}", domain_info.status_string());
        println!();
        println!("Are you sure? This cannot be undone. Use --force to skip this prompt.");
        return Ok(());
    }

    println!("Removing VM '{}'...", opts.name);

    // Remove disk manually if it exists (unmanaged storage)
    if let Some(ref disk_path) = domain_info.disk_path {
        println!("  Removing disk image...");
        if std::path::Path::new(disk_path).exists() {
            std::fs::remove_file(disk_path)
                .with_context(|| format!("Failed to remove disk file: {}", disk_path))?;
        }
    }

    // Remove libvirt domain with nvram
    println!("  Removing libvirt domain...");
    let output = global_opts
        .virsh_command()
        .args(&["undefine", &opts.name, "--nvram"])
        .output()
        .with_context(|| "Failed to undefine libvirt domain")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(color_eyre::eyre::eyre!(
            "Failed to remove libvirt domain: {}",
            stderr
        ));
    }

    println!("VM '{}' removed successfully", opts.name);
    Ok(())
}
