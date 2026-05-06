//! vm ssh — SSH into a running persistent VM.

use super::VmMetadata;
use crate::run_ephemeral_macos::run_ssh_interactive;
use clap::Parser;
use color_eyre::{eyre::bail, Result};

/// Options for `vm ssh`.
#[derive(Parser, Debug)]
pub struct VmSshOpts {
    /// VM name
    pub name: String,
}

/// Open an interactive SSH session to a running persistent VM.
pub fn run(opts: VmSshOpts) -> Result<()> {
    let vm = VmMetadata::load(&opts.name)?;
    if !vm.is_alive() {
        bail!("VM '{}' is not running", opts.name);
    }
    let key_path = std::path::Path::new(&vm.ssh_key);
    run_ssh_interactive(vm.ssh_port, key_path, &vm.ssh_user)?;
    Ok(())
}
