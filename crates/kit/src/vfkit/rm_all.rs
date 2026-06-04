//! vm rm-all — Remove all persistent VMs.

use std::io::Write;

use super::VmMetadata;
use clap::Parser;
use color_eyre::Result;

/// Options for `vm rm-all`.
#[derive(Parser, Debug)]
pub struct VmRmAllOpts {
    /// Force removal without confirmation
    #[clap(long, short = 'f')]
    pub force: bool,
    /// Stop running VMs before removal (gentler than --force kill)
    #[clap(long)]
    pub stop: bool,
    /// Only remove VMs with this label
    #[clap(long)]
    pub label: Option<String>,
}

/// Remove all persistent VMs, with optional label filtering.
pub fn run(opts: VmRmAllOpts) -> Result<()> {
    let mut vms = VmMetadata::list_all()?;

    if let Some(ref filter_label) = opts.label {
        vms.retain(|v| v.labels.contains(filter_label));
    }

    if vms.is_empty() {
        if let Some(ref label) = opts.label {
            println!("No VMs found with label '{}'", label);
        } else {
            println!("No VMs found.");
        }
        return Ok(());
    }

    if !opts.force {
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
        if vm.is_alive() && opts.stop {
            if let Err(e) = super::stop::run(super::stop::VmStopOpts {
                name: vm.name.clone(),
                force: false,
            }) {
                tracing::warn!("failed to stop '{}': {}", vm.name, e);
            }
        }
        let rm_opts = super::rm::VmRmOpts {
            name: vm.name.clone(),
            force: true,
            stop: false,
        };
        super::rm::run(rm_opts)?;
    }
    Ok(())
}
