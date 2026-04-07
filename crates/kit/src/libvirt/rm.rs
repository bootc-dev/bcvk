//! libvirt rm command - remove a bootc domain and its resources
//!
//! This module provides functionality to permanently remove libvirt domains
//! and their associated disk images that were created from bootc container images.

use clap::Parser;
use color_eyre::Result;
use tracing::debug;

/// Check if a domain is persistent (vs transient)
///
/// Returns true if the domain is persistent, false if transient.
/// Transient domains disappear when destroyed, so they don't need undefine.
fn is_domain_persistent(
    global_opts: &crate::libvirt::LibvirtOptions,
    vm_name: &str,
) -> Result<bool> {
    let output = global_opts
        .virsh_command()
        .args(&["dominfo", vm_name])
        .output()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to get domain info: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(color_eyre::eyre::eyre!(
            "Failed to get domain info for '{}': {}",
            vm_name,
            stderr
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Look for "Persistent:     yes" or "Persistent:     no"
    for line in stdout.lines() {
        if let Some(value) = line.strip_prefix("Persistent:") {
            return Ok(value.trim() == "yes");
        }
    }

    // Default to persistent if we can't determine
    Ok(true)
}

/// Options for removing a libvirt domain
#[derive(Debug, Parser)]
pub struct LibvirtRmOpts {
    /// Name of the domain to remove
    pub name: String,

    /// Force removal without confirmation (also stops running VMs)
    #[clap(long, short = 'f')]
    pub force: bool,

    /// Stop domain if it's running (implied by --force)
    #[clap(long)]
    pub stop: bool,
}

/// Core removal implementation that accepts pre-fetched domain state and info
///
/// This private function performs the actual removal logic without fetching
/// domain information, allowing callers to optimize by reusing already-fetched data.
fn remove_vm_impl(
    global_opts: &crate::libvirt::LibvirtOptions,
    vm_name: &str,
    state: &str,
    is_persistent: bool,
    domain_info: &crate::domain_list::PodmanBootcDomain,
    stop_if_running: bool,
) -> Result<()> {
    use color_eyre::eyre::Context;

    // Check if VM is running
    if state == "running" {
        if stop_if_running {
            let output = global_opts
                .virsh_command()
                .args(&["destroy", vm_name])
                .output()
                .with_context(|| "Failed to stop VM before removal")?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(color_eyre::eyre::eyre!(
                    "Failed to stop VM '{}' before removal: {}",
                    vm_name,
                    stderr
                ));
            }

            // Transient VMs disappear after destroy, so we're done
            if !is_persistent {
                return Ok(());
            }
        } else {
            return Err(color_eyre::eyre::eyre!(
                "VM '{}' is running. Cannot remove without stopping.",
                vm_name
            ));
        }
    }

    // Remove disk manually if it exists (unmanaged storage)
    if let Some(ref disk_path) = domain_info.disk_path {
        if std::path::Path::new(disk_path).exists() {
            std::fs::remove_file(disk_path)
                .with_context(|| format!("Failed to remove disk file: {}", disk_path))?;
        }
    }

    // Remove Ignition config file if it exists (stored in metadata)
    // Parse domain XML to get the ignition persistent path
    if let Ok(xml_output) = global_opts
        .virsh_command()
        .args(&["dumpxml", vm_name])
        .output()
    {
        if let Ok(xml_str) = String::from_utf8(xml_output.stdout) {
            if let Ok(dom) = crate::xml_utils::parse_xml_dom(&xml_str) {
                if let Some(ignition_path_node) = dom.find("bootc:ignition-persistent-path") {
                    let ignition_path = ignition_path_node.text_content().trim();
                    if !ignition_path.is_empty() && std::path::Path::new(ignition_path).exists() {
                        debug!("Removing Ignition config file: {}", ignition_path);
                        let _ = std::fs::remove_file(ignition_path); // Don't fail if this fails
                    }
                }
            }
        }
    }

    // Remove libvirt domain with nvram and storage
    let output = global_opts
        .virsh_command()
        .args(&["undefine", vm_name, "--nvram", "--remove-all-storage"])
        .output()
        .with_context(|| "Failed to undefine libvirt domain")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(color_eyre::eyre::eyre!(
            "Failed to remove libvirt domain: {}",
            stderr
        ));
    }

    Ok(())
}

/// Remove a VM without confirmation
///
/// This is the core removal logic that can be reused by other commands.
/// It assumes the caller has already confirmed the operation.
pub fn remove_vm_forced(
    global_opts: &crate::libvirt::LibvirtOptions,
    vm_name: &str,
    stop_if_running: bool,
) -> Result<()> {
    use crate::domain_list::DomainLister;
    use color_eyre::eyre::Context;

    let connect_uri = global_opts.connect.as_ref();
    let lister = match connect_uri {
        Some(uri) => DomainLister::with_connection(uri.clone()),
        None => DomainLister::new(),
    };

    // Check if domain exists and get its state
    let state = match lister.get_domain_state(vm_name) {
        Ok(s) => s,
        Err(_) => {
            // Domain doesn't exist - this is OK for replace scenarios
            // where a transient VM was already destroyed
            return Ok(());
        }
    };

    // Check if domain is persistent (transient VMs disappear after destroy)
    let is_persistent = is_domain_persistent(global_opts, vm_name)?;

    // Get domain info for disk cleanup
    let domain_info = lister
        .get_domain_info(vm_name)
        .with_context(|| format!("Failed to get info for VM '{}'", vm_name))?;

    remove_vm_impl(
        global_opts,
        vm_name,
        &state,
        is_persistent,
        &domain_info,
        stop_if_running,
    )
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

    // Check if domain is persistent (transient VMs disappear after destroy)
    let is_persistent = is_domain_persistent(global_opts, &opts.name)?;

    // Get domain info for display
    let domain_info = lister
        .get_domain_info(&opts.name)
        .with_context(|| format!("Failed to get info for VM '{}'", opts.name))?;

    // Check if VM is running
    if state == "running" {
        // --force implies --stop
        if opts.stop || opts.force {
            println!("Stopping running VM '{}'...", opts.name);
        } else {
            return Err(color_eyre::eyre::eyre!(
                "VM '{}' is running. Use --stop or --force to remove a running VM, or stop it first.",
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

    // Use the optimized removal implementation with already-fetched info
    remove_vm_impl(
        global_opts,
        &opts.name,
        &state,
        is_persistent,
        &domain_info,
        opts.stop || opts.force,
    )?;

    println!("VM '{}' removed successfully", opts.name);
    Ok(())
}
