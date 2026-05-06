//! Ephemeral VM management commands for macOS (vfkit backend).

use std::io::Write;
use std::process::{Command, Stdio};

use clap::Subcommand;
use color_eyre::eyre::bail;
use color_eyre::Result;

use crate::run_ephemeral_macos::{self, EphemeralVmMetadata};

/// Options for `ephemeral run-ssh`, combining run options with optional SSH arguments.
#[derive(Debug, clap::Parser)]
pub struct RunSshOpts {
    #[command(flatten)]
    pub run_opts: run_ephemeral_macos::RunEphemeralOpts,

    /// SSH command to execute (optional, defaults to interactive shell)
    #[arg(trailing_var_arg = true)]
    pub ssh_args: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum EphemeralCommands {
    /// Run bootc containers as ephemeral VMs
    #[clap(name = "run")]
    Run(run_ephemeral_macos::RunEphemeralOpts),

    /// Run ephemeral VM and SSH into it
    #[clap(name = "run-ssh")]
    RunSsh(RunSshOpts),

    /// Connect to a running ephemeral VM via SSH
    #[clap(name = "ssh")]
    Ssh {
        /// VM name
        name: String,

        /// Additional SSH arguments (e.g. -v, -L, commands to execute)
        #[clap(allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// List ephemeral VM containers
    #[clap(name = "ps")]
    Ps {
        /// Output as JSON
        #[clap(long)]
        json: bool,
    },

    /// Remove all ephemeral VM containers
    #[clap(name = "rm-all")]
    RmAll {
        /// Force removal without confirmation
        #[clap(short, long)]
        force: bool,
    },
}

impl EphemeralCommands {
    /// Execute the ephemeral subcommand.
    pub fn run(self) -> Result<()> {
        match self {
            EphemeralCommands::Run(opts) => run_ephemeral_macos::run(opts),
            EphemeralCommands::RunSsh(mut opts) => {
                opts.run_opts.ssh_keygen = true;
                if !opts.ssh_args.is_empty() {
                    let combined = shlex::try_join(opts.ssh_args.iter().map(|s| s.as_str()))
                        .map_err(|e| color_eyre::eyre::eyre!("failed to escape SSH args: {}", e))?;
                    opts.run_opts.execute.push(combined);
                }
                run_ephemeral_macos::run(opts.run_opts)
            }
            EphemeralCommands::Ssh { name, args } => cmd_ssh(&name, &args),
            EphemeralCommands::Ps { json } => cmd_ps(json),
            EphemeralCommands::RmAll { force } => cmd_rm_all(force),
        }
    }
}

fn cmd_ps(json: bool) -> Result<()> {
    let vms = EphemeralVmMetadata::list_all()?;
    for vm in &vms {
        if !vm.is_alive() {
            EphemeralVmMetadata::remove(&vm.name);
        }
    }
    let live: Vec<_> = vms.into_iter().filter(|vm| vm.is_alive()).collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&live)?);
        return Ok(());
    }

    if live.is_empty() {
        println!("No running ephemeral VMs.");
        return Ok(());
    }

    println!("{:<24} {:<50} SSH", "NAME", "IMAGE");
    for vm in &live {
        println!(
            "{:<24} {:<50} ssh -p {} -i {} root@localhost",
            vm.name, vm.image, vm.ssh_port, vm.ssh_key
        );
    }
    Ok(())
}

fn cmd_rm_all(force: bool) -> Result<()> {
    let vms = EphemeralVmMetadata::list_all()?;
    if vms.is_empty() {
        println!("No ephemeral VMs found.");
        return Ok(());
    }

    if !force {
        println!("Found {} ephemeral VM(s):", vms.len());
        for vm in &vms {
            println!(
                "  {} ({})",
                vm.name,
                if vm.is_alive() { "running" } else { "stopped" }
            );
        }
        print!("Remove all ephemeral VMs? [y/N]: ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();
        if input != "y" && input != "yes" {
            println!("Aborted.");
            return Ok(());
        }
    }

    for vm in &vms {
        if vm.is_alive() {
            if let Err(e) = Command::new("kill")
                .args([&vm.pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
            {
                tracing::warn!("failed to kill VM process {}: {}", vm.pid, e);
            }
            if vm.gvproxy_pid > 0 {
                if let Err(e) = Command::new("kill")
                    .args([&vm.gvproxy_pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                {
                    tracing::warn!("failed to kill gvproxy {}: {}", vm.gvproxy_pid, e);
                }
            }
        }
        EphemeralVmMetadata::remove(&vm.name);
        println!("Removed {}", vm.name);
    }
    Ok(())
}

fn cmd_ssh(name: &str, args: &[String]) -> Result<()> {
    let vm = EphemeralVmMetadata::load(name)?;
    if !vm.is_alive() {
        EphemeralVmMetadata::remove(name);
        bail!("VM '{}' is not running", name);
    }

    // Try to set up SSH port forwarding via VM-specific gvproxy socket
    let svc_sock = format!("/private/tmp/bcvk/{}-gvproxy-svc.sock", name);
    if std::path::Path::new(&svc_sock).exists() {
        if let Err(e) =
            run_ephemeral_macos::expose_ssh_port(&svc_sock, "192.168.127.2", vm.ssh_port)
        {
            tracing::debug!("SSH port forward re-expose: {}", e);
        }
    }

    let key_path = std::path::Path::new(&vm.ssh_key);
    if args.is_empty() {
        run_ephemeral_macos::run_ssh_interactive(vm.ssh_port, key_path, "root")?;
    } else {
        let combined = shlex::try_join(args.iter().map(|s| s.as_str()))
            .map_err(|e| color_eyre::eyre::eyre!("failed to escape SSH command: {}", e))?;
        let status =
            run_ephemeral_macos::run_ssh_command(vm.ssh_port, key_path, "root", &combined)?;
        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }
    }
    Ok(())
}
