//! libvirt list command - list bootc domains
//!
//! This module provides functionality to list libvirt domains that were
//! created from bootc container images, showing their status and metadata.

use clap::Parser;
use color_eyre::Result;
use comfy_table::{presets::UTF8_FULL, Table};

use super::OutputFormat;

/// Options for listing libvirt domains
#[derive(Debug, Parser)]
pub struct LibvirtListOpts {
    /// Domain name to query (returns only this domain)
    pub domain_name: Option<String>,

    /// Output format
    #[clap(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,

    /// Show all domains including stopped ones
    #[clap(long, short = 'a')]
    pub all: bool,

    /// Filter domains by label
    #[clap(long)]
    pub label: Option<String>,
}

/// Execute the libvirt list command
pub fn run(global_opts: &crate::libvirt::LibvirtOptions, opts: LibvirtListOpts) -> Result<()> {
    use crate::domain_list::DomainLister;
    use color_eyre::eyre::Context;

    // Use libvirt as the source of truth for domain listing
    let connect_uri = global_opts.connect.as_ref();
    let lister = match connect_uri {
        Some(uri) => DomainLister::with_connection(uri.clone()),
        None => DomainLister::new(),
    };

    let mut domains = if let Some(ref domain_name) = opts.domain_name {
        // Query specific domain by name
        match lister.get_domain_info(domain_name) {
            Ok(domain) => vec![domain],
            Err(e) => {
                return Err(color_eyre::eyre::eyre!(
                    "Failed to get domain '{}': {}",
                    domain_name,
                    e
                ));
            }
        }
    } else if opts.all {
        lister
            .list_bootc_domains()
            .with_context(|| "Failed to list bootc domains from libvirt")?
    } else {
        lister
            .list_running_bootc_domains()
            .with_context(|| "Failed to list running bootc domains from libvirt")?
    };

    // Filter by label if specified
    if let Some(ref filter_label) = opts.label {
        domains.retain(|d| d.labels.contains(filter_label));
    }

    match opts.format {
        OutputFormat::Table => {
            if domains.is_empty() {
                if opts.all {
                    println!("No VMs found");
                    println!("Tip: Create VMs with 'bcvk libvirt run <image>'");
                } else {
                    println!("No running VMs found");
                    println!(
                        "Use --all to see stopped VMs or 'bcvk libvirt run <image>' to create one"
                    );
                }
                return Ok(());
            }

            let mut table = Table::new();
            table.load_preset(UTF8_FULL);
            table.set_header(vec!["NAME", "IMAGE", "STATUS", "MEMORY", "SSH"]);

            for domain in &domains {
                let image = match &domain.image {
                    Some(img) => {
                        if img.len() > 38 {
                            format!("{}...", &img[..35])
                        } else {
                            img.clone()
                        }
                    }
                    None => "<no metadata>".to_string(),
                };
                let memory = match domain.memory_mb {
                    Some(mem) => format!("{}MB", mem),
                    None => "unknown".to_string(),
                };
                let ssh = match domain.ssh_port {
                    Some(port) if domain.has_ssh_key => format!(":{}", port),
                    Some(port) => format!(":{}*", port),
                    None => "-".to_string(),
                };
                table.add_row(vec![
                    &domain.name,
                    &image,
                    &domain.status_string(),
                    &memory,
                    &ssh,
                ]);
            }

            println!("{}", table);
            println!(
                "\nFound {} domain{} (source: libvirt)",
                domains.len(),
                if domains.len() == 1 { "" } else { "s" }
            );
        }
        OutputFormat::Json => {
            // If querying a specific domain, return object directly instead of array
            if opts.domain_name.is_some() && !domains.is_empty() {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&domains[0])
                        .with_context(|| "Failed to serialize domain as JSON")?
                );
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&domains)
                        .with_context(|| "Failed to serialize domains as JSON")?
                );
            }
        }
        OutputFormat::Yaml => {
            return Err(color_eyre::eyre::eyre!(
                "YAML format is not supported for list command"
            ))
        }
    }
    Ok(())
}
