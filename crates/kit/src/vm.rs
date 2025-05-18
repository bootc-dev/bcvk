//! Module for VM-related functionality
//!
//! This contains common code used by both run-rmvm and virt-install commands

use bootc_utils::CommandRunExt;
use color_eyre::{eyre::eyre, Result};
use tracing::instrument;

use crate::hostexec;

/// Check if libvirt is available on the host
#[instrument]
pub fn check_libvirt_available() -> Result<bool> {
    let status = hostexec::command("systemctl", None)?
        .args(["is-active", "libvirtd"])
        .output()
        .map_err(|e| eyre!("Checking if libvirtd is running: {}", e))?;

    Ok(status.status.success())
}

/// Check if a VM exists by name
#[instrument]
pub fn vm_exists(name: &str) -> Result<bool> {
    let output = hostexec::command("virsh", None)?
        .args(["list", "--all", "--name"])
        .output()
        .map_err(|e| eyre!("Listing VMs: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().any(|line| line.trim() == name))
}

/// Get the state of a VM
#[instrument]
pub fn get_vm_state(name: &str) -> Result<String> {
    let output = hostexec::command("virsh", None)?
        .args(["domstate", name])
        .output()
        .map_err(|e| eyre!("Getting VM state: {}", e))?;

    if !output.status.success() {
        return Err(eyre!("Failed to get state for VM {}", name));
    }

    let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(state)
}

/// Start a VM if it's not running
#[instrument]
pub fn ensure_vm_running(name: &str) -> Result<()> {
    // Check if the VM exists
    if !vm_exists(name)? {
        return Err(eyre!("VM {} does not exist", name));
    }

    // Check the VM state
    let state = get_vm_state(name)?;
    if state == "running" {
        return Ok(());
    }

    // Start the VM
    println!("Starting VM {}...", name);
    hostexec::command("virsh", None)?
        .args(["start", name])
        .run()
        .map_err(|e| eyre!("Starting VM {}: {}", name, e))?;

    Ok(())
}

/// Stop a VM
#[instrument]
pub fn stop_vm(name: &str, force: bool) -> Result<()> {
    // Check if the VM exists
    if !vm_exists(name)? {
        return Err(eyre!("VM {} does not exist", name));
    }

    // Check the VM state
    let state = get_vm_state(name)?;
    if state != "running" {
        return Ok(());
    }

    // Stop the VM
    if force {
        println!("Forcing VM {} to stop...", name);
        hostexec::command("virsh", None)?
            .args(["destroy", name])
            .run()
            .map_err(|e| eyre!("Forcing VM {} to stop: {}", name, e))?;
    } else {
        println!("Shutting down VM {}...", name);
        hostexec::command("virsh", None)?
            .args(["shutdown", name])
            .run()
            .map_err(|e| eyre!("Shutting down VM {}: {}", name, e))?;
    }

    Ok(())
}

/// Delete a VM
#[instrument]
pub fn delete_vm(name: &str) -> Result<()> {
    // Check if the VM exists
    if !vm_exists(name)? {
        return Err(eyre!("VM {} does not exist", name));
    }

    // If the VM is running, stop it first
    let state = get_vm_state(name)?;
    if state == "running" {
        stop_vm(name, true)?;
    }

    // Undefine the VM
    println!("Deleting VM {}...", name);
    hostexec::command("virsh", None)?
        .args(["undefine", name, "--remove-all-storage"])
        .run()
        .map_err(|e| eyre!("Deleting VM {}: {}", name, e))?;

    Ok(())
}

/// Get IP address of a VM
#[instrument]
pub fn get_vm_ip(name: &str) -> Result<Option<String>> {
    let output = hostexec::command("virsh", None)?
        .args(["domifaddr", name])
        .output()
        .map_err(|e| eyre!("Getting VM IP address: {}", e))?;

    if !output.status.success() {
        return Err(eyre!("Failed to get IP address for VM {}", name));
    }

    // Parse the output to find the IP address
    // Example output:
    // Name       MAC address          Protocol     Address
    // -------------------------------------------------------
    // vnet0      52:54:00:01:02:03    ipv4         192.168.122.2/24

    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines().skip(2) {
        // Skip header lines
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            let addr = parts[3];
            if let Some((ip, _)) = addr.split_once('/') {
                return Ok(Some(ip.to_string()));
            }
        }
    }

    Ok(None) // No IP address found
}
