//! vm ssh — SSH into a running persistent VM.

use super::{VmMetadata, VmMetadataStore};
use crate::vm_helpers::{run_ssh_command, run_ssh_interactive};
use clap::Parser;
use color_eyre::{eyre::bail, Result};

/// Options for `vm ssh`.
#[derive(Parser, Debug)]
pub struct VmSshOpts {
    /// VM name
    pub name: String,
    /// SSH username to use for connection (defaults to 'root')
    #[clap(long, default_value = "root")]
    pub user: String,
    /// Additional SSH arguments
    #[clap(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// Open an SSH session to a running persistent VM.
pub fn run(opts: VmSshOpts) -> Result<()> {
    let vm = VmMetadata::load(&opts.name)?;
    if !vm.is_alive() {
        bail!("VM '{}' is not running", opts.name);
    }
    let key_path = std::path::Path::new(&vm.ssh_key);
    if opts.args.is_empty() {
        run_ssh_interactive(vm.ssh_port, key_path, &opts.user)?;
    } else {
        let cmd = shlex::try_join(opts.args.iter().map(|s| s.as_str()))
            .map_err(|e| color_eyre::eyre::eyre!("failed to escape SSH args: {}", e))?;
        let status = run_ssh_command(vm.ssh_port, key_path, &opts.user, &cmd)?;
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
