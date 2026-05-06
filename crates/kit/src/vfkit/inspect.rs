//! vm inspect — Show detailed VM information.

use super::VmMetadata;
use color_eyre::Result;

/// Display detailed metadata for the named VM.
pub fn run(name: &str, json: bool) -> Result<()> {
    let meta = VmMetadata::load(name)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&meta)?);
        return Ok(());
    }

    let state = if meta.is_alive() {
        "running"
    } else {
        "stopped"
    };

    println!("Name:       {}", meta.name);
    println!("State:      {}", state);
    println!("Disk:       {}", meta.disk_image);
    println!("CPUs:       {}", meta.cpus);
    println!("Memory:     {} MiB", meta.memory);
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
    println!("  User:     {}", meta.ssh_user);
    println!("  Key:      {}", meta.ssh_key);
    if state == "running" {
        println!();
        println!(
            "  ssh -p {} -i {} {}@localhost",
            meta.ssh_port, meta.ssh_key, meta.ssh_user
        );
    }
    println!();
    println!("Files:");
    println!("  EFI store:  {}", meta.efi_store);
    println!("  Serial log: {}", meta.serial_log);

    Ok(())
}
