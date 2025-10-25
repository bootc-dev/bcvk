//! Implementation of `bcvk project down` command

use clap::Parser;
use color_eyre::Result;

use crate::libvirt::{self, LibvirtOptions};

use super::{current_project_dir, project_vm_name};

/// Shut down the project VM
///
/// Gracefully shuts down the VM but does not remove it.
#[derive(Debug, Parser)]
pub struct ProjectDownOpts {
    /// Libvirt connection URI (defaults to qemu:///session)
    #[clap(long)]
    pub connect: Option<String>,

    /// Remove the VM after shutting it down
    #[clap(long)]
    pub remove: bool,
}

/// Run the project down command
pub fn run(opts: ProjectDownOpts) -> Result<()> {
    // Get current project directory
    let project_dir = current_project_dir()?;

    // Load project configuration (optional for down, just for name generation)
    let config = crate::project::config::ProjectConfig::load_from_dir(&project_dir)?;

    // Generate project VM name
    let vm_name = project_vm_name(&project_dir, config.as_ref())?;

    // Check if VM exists
    let libvirt_opts = LibvirtOptions {
        connect: opts.connect.clone(),
    };

    if !check_vm_exists(&vm_name, &libvirt_opts)? {
        println!("Project VM '{}' does not exist", vm_name);
        return Ok(());
    }

    // Stop the VM
    println!("Shutting down project VM '{}'...", vm_name);
    stop_vm(&vm_name, &libvirt_opts)?;

    // Remove if requested
    if opts.remove {
        println!("Removing project VM '{}'...", vm_name);
        remove_vm(&vm_name, &libvirt_opts)?;
    }

    Ok(())
}

/// Check if a VM exists
fn check_vm_exists(name: &str, libvirt_opts: &LibvirtOptions) -> Result<bool> {
    use crate::domain_list::DomainLister;

    let lister = if let Some(ref uri) = libvirt_opts.connect {
        DomainLister::with_connection(uri.clone())
    } else {
        DomainLister::new()
    };
    let domains = lister.list_bootc_domains()?;

    Ok(domains.iter().any(|d| d.name == name))
}

/// Stop a VM
fn stop_vm(name: &str, libvirt_opts: &LibvirtOptions) -> Result<()> {
    let stop_opts = libvirt::stop::LibvirtStopOpts {
        name: name.to_string(),
        force: false,
        timeout: 60,
    };

    libvirt::stop::run(libvirt_opts, stop_opts)
}

/// Remove a VM
fn remove_vm(name: &str, libvirt_opts: &LibvirtOptions) -> Result<()> {
    let rm_opts = libvirt::rm::LibvirtRmOpts {
        name: name.to_string(),
        force: false,
        stop: false,
    };

    libvirt::rm::run(libvirt_opts, rm_opts)
}
