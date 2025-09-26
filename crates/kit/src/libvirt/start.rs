//! libvirt start command - start a stopped bootc domain
//!
//! This module provides functionality to start stopped libvirt domains
//! that were created from bootc container images.

use clap::Parser;
use color_eyre::Result;

/// Options for starting a libvirt domain
#[derive(Debug, Parser)]
pub struct LibvirtStartOpts {
    /// Name of the domain to start
    pub name: String,

    /// Automatically SSH into the domain after starting
    #[clap(long)]
    pub ssh: bool,
}

/// Execute the libvirt start command
pub fn run(global_opts: &crate::libvirt::LibvirtOptions, opts: LibvirtStartOpts) -> Result<()> {
    use crate::domain_list::DomainLister;
    use color_eyre::eyre::Context;

    let connect_uri = global_opts.connect.as_ref();
    let lister = match connect_uri {
        Some(uri) => DomainLister::with_connection(uri.clone()),
        None => DomainLister::new(),
    };

    // Check if domain exists and get its state
    let state = lister
        .get_domain_state(&opts.name)
        .map_err(|_| color_eyre::eyre::eyre!("VM '{}' not found", opts.name))?;

    if state == "running" {
        println!("VM '{}' is already running", opts.name);
        if opts.ssh {
            println!("🔗 Connecting to running VM...");
            let ssh_opts = crate::libvirt::ssh::LibvirtSshOpts {
                domain_name: opts.name,
                user: "root".to_string(),
                command: vec![],
                strict_host_keys: false,
                timeout: 30,
                log_level: "ERROR".to_string(),
                extra_options: vec![],
            };
            return crate::libvirt::ssh::run(global_opts, ssh_opts);
        }
        return Ok(());
    }

    println!("Starting VM '{}'...", opts.name);

    // Use virsh to start the domain
    let output = global_opts
        .virsh_command()
        .args(&["start", &opts.name])
        .output()
        .with_context(|| "Failed to run virsh start")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(color_eyre::eyre::eyre!(
            "Failed to start VM '{}': {}",
            opts.name,
            stderr
        ));
    }

    println!("VM '{}' started successfully", opts.name);

    if opts.ssh {
        // Use the libvirt SSH functionality directly
        let ssh_opts = crate::libvirt::ssh::LibvirtSshOpts {
            domain_name: opts.name,
            user: "root".to_string(),
            command: vec![],
            strict_host_keys: false,
            timeout: 30,
            log_level: "ERROR".to_string(),
            extra_options: vec![],
        };
        crate::libvirt::ssh::run(global_opts, ssh_opts)
    } else {
        Ok(())
    }
}
