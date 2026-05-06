//! vm list — List all persistent VMs.

use super::VmMetadata;
use color_eyre::Result;

/// List all persistent VMs, optionally as JSON.
pub fn run(json: bool) -> Result<()> {
    let vms = VmMetadata::list_all()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&vms)?);
        return Ok(());
    }

    if vms.is_empty() {
        println!("No VMs found.");
        return Ok(());
    }

    println!("{:<20} {:<10} {:<30} SSH", "NAME", "STATE", "DISK");
    for vm in &vms {
        let state = if vm.is_alive() { "running" } else { "stopped" };
        println!(
            "{:<20} {:<10} {:<30} ssh -p {} -i {} {}@localhost",
            vm.name, state, vm.disk_image, vm.ssh_port, vm.ssh_key, vm.ssh_user
        );
    }
    Ok(())
}
