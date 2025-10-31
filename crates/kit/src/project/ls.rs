//! Implementation of `bcvk project ls` command

use clap::Parser;
use color_eyre::Result;
use comfy_table::{presets::UTF8_FULL, Table};

use crate::libvirt::OutputFormat;

/// List all project VMs with their metadata
#[derive(Debug, Parser)]
pub struct ProjectLsOpts {
    /// Libvirt connection URI (defaults to qemu:///session)
    #[clap(long)]
    pub connect: Option<String>,

    /// Output format
    #[clap(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,

    /// Show all VMs including stopped ones
    #[clap(long, short = 'a')]
    pub all: bool,
}

/// Run the project ls command
pub fn run(opts: ProjectLsOpts) -> Result<()> {
    use crate::domain_list::DomainLister;
    use color_eyre::eyre::Context;

    // Use libvirt as the source of truth for domain listing
    let lister = match opts.connect.as_ref() {
        Some(uri) => DomainLister::with_connection(uri.clone()),
        None => DomainLister::new(),
    };

    let mut domains = if opts.all {
        lister
            .list_bootc_domains()
            .with_context(|| "Failed to list bootc domains from libvirt")?
    } else {
        lister
            .list_running_bootc_domains()
            .with_context(|| "Failed to list running bootc domains from libvirt")?
    };

    // Filter to only project VMs (those with bcvk-project label)
    domains.retain(|d| d.labels.contains(&"bcvk-project".to_string()));

    match opts.format {
        OutputFormat::Table => {
            if domains.is_empty() {
                if opts.all {
                    println!("No project VMs found");
                    println!("Tip: Create a project VM with 'bcvk project up'");
                } else {
                    println!("No running project VMs found");
                    println!("Use --all to see stopped VMs or 'bcvk project up' to create one");
                }
                return Ok(());
            }

            let mut table = Table::new();
            table.load_preset(UTF8_FULL);
            table.set_header(vec![
                "NAME",
                "PROJECT DIR",
                "IMAGE",
                "STATUS",
                "MEMORY",
                "SSH",
            ]);

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
                let project_dir = domain
                    .project_dir
                    .as_ref()
                    .map(|p| p.as_str())
                    .unwrap_or("<no metadata>");
                table.add_row(vec![
                    &domain.name,
                    project_dir,
                    &image,
                    &domain.status_string(),
                    &memory,
                    &ssh,
                ]);
            }

            println!("{}", table);
            println!(
                "\nFound {} project VM{} (source: libvirt)",
                domains.len(),
                if domains.len() == 1 { "" } else { "s" }
            );
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&domains)
                    .with_context(|| "Failed to serialize domains as JSON")?
            );
        }
        OutputFormat::Yaml => {
            return Err(color_eyre::eyre::eyre!(
                "YAML format is not supported for list command"
            ))
        }
    }
    Ok(())
}
