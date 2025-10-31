//! libvirt inspect command - show detailed information about a bootc domain
//!
//! This module provides functionality to display detailed information about
//! libvirt domains that were created from bootc container images.

use clap::Parser;
use color_eyre::Result;

use super::OutputFormat;

/// Options for inspecting a libvirt domain
#[derive(Debug, Parser)]
pub struct LibvirtInspectOpts {
    /// Name of the domain to inspect
    pub name: String,

    /// Output format
    #[clap(long, value_enum, default_value_t = OutputFormat::Yaml)]
    pub format: OutputFormat,
}

/// Execute the libvirt inspect command
pub fn run(global_opts: &crate::libvirt::LibvirtOptions, opts: LibvirtInspectOpts) -> Result<()> {
    use crate::domain_list::DomainLister;
    use color_eyre::eyre::Context;

    let connect_uri = global_opts.connect.as_ref();
    let lister = match connect_uri {
        Some(uri) => DomainLister::with_connection(uri.clone()),
        None => DomainLister::new(),
    };

    // Get domain info
    let vm = lister
        .get_domain_info(&opts.name)
        .map_err(|_| color_eyre::eyre::eyre!("VM '{}' not found", opts.name))?;

    match opts.format {
        OutputFormat::Yaml => {
            println!("name: {}", vm.name);
            if let Some(ref image) = vm.image {
                println!("image: {}", image);
            }
            println!("status: {}", vm.status_string());
            if let Some(memory) = vm.memory_mb {
                println!("memory_mb: {}", memory);
            }
            if let Some(vcpus) = vm.vcpus {
                println!("vcpus: {}", vcpus);
            }
            if let Some(ref disk_path) = vm.disk_path {
                println!("disk_path: {}", disk_path);
            }
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&vm)
                    .with_context(|| "Failed to serialize VM as JSON")?
            );
        }
        OutputFormat::Xml => {
            // Output raw domain XML using virsh dumpxml
            let mut cmd = global_opts.virsh_command();
            cmd.args(["dumpxml", &opts.name]);
            let output = cmd
                .output()
                .with_context(|| format!("Failed to run virsh dumpxml for {}", opts.name))?;

            if !output.status.success() {
                return Err(color_eyre::eyre::eyre!(
                    "Failed to get domain XML: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }

            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
        OutputFormat::Table => {
            return Err(color_eyre::eyre::eyre!(
                "Table format is not supported for inspect command"
            ))
        }
    }
    Ok(())
}
