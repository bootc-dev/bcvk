//! vm rm-all — Remove all persistent VMs.

use std::io::Write;

use super::VmMetadata;
use color_eyre::Result;

/// Remove all persistent VMs, prompting unless `force` is set.
pub fn run(force: bool) -> Result<()> {
    let vms = VmMetadata::list_all()?;
    if vms.is_empty() {
        println!("No VMs found.");
        return Ok(());
    }

    if !force {
        println!("Found {} VM(s):", vms.len());
        for vm in &vms {
            println!(
                "  {} ({})",
                vm.name,
                if vm.is_alive() { "running" } else { "stopped" }
            );
        }
        print!("Remove all VMs? [y/N]: ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();
        if input != "y" && input != "yes" {
            println!("Aborted.");
            return Ok(());
        }
    }

    for vm in &vms {
        let opts = super::rm::VmRmOpts {
            name: vm.name.clone(),
            force: true,
        };
        super::rm::run(opts)?;
    }
    Ok(())
}
