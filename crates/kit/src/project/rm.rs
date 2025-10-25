//! Implementation of `bcvk project rm` command

use clap::Parser;
use color_eyre::Result;

use crate::libvirt::{self, LibvirtOptions};

use super::{current_project_dir, project_vm_name};

/// Remove the project VM and its resources
///
/// Permanently removes the VM and its associated disk images.
#[derive(Debug, Parser)]
pub struct ProjectRmOpts {
    /// Libvirt connection URI (defaults to qemu:///session)
    #[clap(long)]
    pub connect: Option<String>,

    /// Force removal without confirmation
    #[clap(long, short = 'f')]
    pub force: bool,

    /// Remove domain even if it's running
    #[clap(long)]
    pub stop: bool,
}

/// Run the project rm command
pub fn run(opts: ProjectRmOpts) -> Result<()> {
    // Get current project directory
    let project_dir = current_project_dir()?;

    // Load project configuration (optional for rm, just for name generation)
    let config = crate::project::config::ProjectConfig::load_from_dir(&project_dir)?;

    // Generate project VM name
    let vm_name = project_vm_name(&project_dir, config.as_ref())?;

    // Build libvirt options
    let libvirt_opts = LibvirtOptions {
        connect: opts.connect,
    };

    // Build libvirt rm options
    let rm_opts = libvirt::rm::LibvirtRmOpts {
        name: vm_name,
        force: opts.force,
        stop: opts.stop,
    };

    // Delegate to libvirt rm
    libvirt::rm::run(&libvirt_opts, rm_opts)
}
