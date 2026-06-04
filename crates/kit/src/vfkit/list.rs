//! vm list — List all persistent VMs.

use super::{OutputFormat, VmMetadata};
use clap::Parser;
use color_eyre::Result;

/// Options for `vm list`.
#[derive(Parser, Debug)]
pub struct VmListOpts {
    /// VM name to query (returns only this VM)
    pub domain_name: Option<String>,
    /// Output format
    #[clap(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
    /// Show all VMs including stopped ones
    #[clap(long, short = 'a')]
    pub all: bool,
    /// Filter VMs by label
    #[clap(long)]
    pub label: Option<String>,
}

/// List persistent VMs with optional filtering and format selection.
pub fn run(opts: VmListOpts) -> Result<()> {
    let all_vms = if let Some(ref name) = opts.domain_name {
        match VmMetadata::load(name) {
            Ok(meta) => vec![meta],
            Err(e) => {
                return Err(color_eyre::eyre::eyre!(
                    "Failed to get VM '{}': {}",
                    name,
                    e
                ));
            }
        }
    } else {
        VmMetadata::list_all()?
    };

    let mut vms: Vec<_> = all_vms
        .into_iter()
        .filter(|vm| opts.all || opts.domain_name.is_some() || vm.is_alive())
        .collect();

    if let Some(ref filter_label) = opts.label {
        vms.retain(|vm| vm.labels.contains(filter_label));
    }

    match opts.format {
        OutputFormat::Table => {
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
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&vms)?);
        }
        OutputFormat::Yaml => {
            for vm in &vms {
                let state = if vm.is_alive() { "running" } else { "stopped" };
                println!("- name: {}", vm.name);
                println!("  state: {}", state);
                println!("  disk: {}", vm.disk_image);
                println!("  vcpus: {}", vm.vcpus);
                println!("  memory_mb: {}", vm.memory_mb);
                println!("  ssh_port: {}", vm.ssh_port);
            }
        }
        OutputFormat::Xml => {
            return Err(color_eyre::eyre::eyre!(
                "XML format is not supported for list command"
            ));
        }
    }
    Ok(())
}
