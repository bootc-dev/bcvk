//! vm inspect — Show detailed VM information.

use super::{OutputFormat, VmMetadata};
use clap::Parser;
use color_eyre::Result;

/// Options for `vm inspect`.
#[derive(Parser, Debug)]
pub struct VmInspectOpts {
    /// VM name
    pub name: String,
    /// Output format
    #[clap(long, value_enum, default_value_t = OutputFormat::Yaml)]
    pub format: OutputFormat,
}

/// Display detailed metadata for the named VM.
pub fn run(opts: VmInspectOpts) -> Result<()> {
    let meta = VmMetadata::load(&opts.name)?;

    match opts.format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&meta)?);
            return Ok(());
        }
        OutputFormat::Yaml | OutputFormat::Table => {}
        OutputFormat::Xml => {
            return Err(color_eyre::eyre::eyre!(
                "XML format is not supported for inspect command"
            ));
        }
    }

    let state = if meta.is_alive() {
        "running"
    } else {
        "stopped"
    };

    println!("Name:       {}", meta.name);
    println!("State:      {}", state);
    println!("Disk:       {}", meta.disk_image);
    println!("CPUs:       {}", meta.vcpus);
    println!("Memory:     {} MiB", meta.memory_mb);
    println!("GUI:        {}", meta.gui);
    println!("Created:    {}", meta.created);
    println!();
    println!("Processes:");
    if meta.vfkit_pid > 0 {
        println!(
            "  vfkit:    PID {} ({})",
            meta.vfkit_pid,
            if meta.is_alive() {
                "running"
            } else {
                "stopped"
            }
        );
    }
    if meta.gvproxy_pid > 0 {
        println!("  gvproxy:  PID {}", meta.gvproxy_pid);
    }
    println!();
    println!("SSH:");
    println!("  Port:     {}", meta.ssh_port);
    println!("  Key:      {}", meta.ssh_key);
    if state == "running" {
        println!();
        println!(
            "  ssh -p {} -i {} root@localhost",
            meta.ssh_port, meta.ssh_key
        );
    }
    if !meta.labels.is_empty() {
        println!();
        println!("Labels:     {}", meta.labels.join(", "));
    }
    if !meta.port_mappings.is_empty() {
        println!();
        println!("Port mappings:");
        for (h, g) in &meta.port_mappings {
            println!("  {}:{}", h, g);
        }
    }
    println!();
    println!("Files:");
    println!("  EFI store:  {}", meta.efi_store);
    println!("  Serial log: {}", meta.serial_log);

    Ok(())
}
