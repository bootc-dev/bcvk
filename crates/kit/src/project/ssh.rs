//! Implementation of `bcvk project ssh` command

use clap::Parser;
use color_eyre::Result;

use crate::libvirt::{self, LibvirtOptions};

use super::{current_project_dir, project_vm_name};

/// SSH into the project VM
///
/// Automatically starts the VM if it's stopped.
#[derive(Debug, Parser)]
pub struct ProjectSshOpts {
    /// Libvirt connection URI (defaults to qemu:///session)
    #[clap(long)]
    pub connect: Option<String>,

    /// Run bootc upgrade in two stages (fetch/stage, then apply/reboot) before connecting
    #[clap(long, short = 'A')]
    pub update: bool,

    /// Command to execute in the VM (if empty, opens interactive shell)
    #[clap(allow_hyphen_values = true)]
    pub command: Vec<String>,
}

/// Run the project ssh command
pub fn run(opts: ProjectSshOpts) -> Result<()> {
    // Get current project directory
    let project_dir = current_project_dir()?;

    // Load project configuration (optional for ssh, just for name generation)
    let config = crate::project::config::ProjectConfig::load_from_dir(&project_dir)?;

    // Generate project VM name
    let vm_name = project_vm_name(&project_dir, config.as_ref())?;

    // Check VM state and start if needed
    let libvirt_opts = LibvirtOptions {
        connect: opts.connect.clone(),
    };

    ensure_vm_running(&vm_name, &libvirt_opts)?;

    // If --update is requested, run bootc upgrade in two stages
    if opts.update {
        // Stage 1: Fetch and prepare the update (without reboot)
        println!("Running bootc upgrade (fetching and staging update)...");
        let upgrade_opts = libvirt::ssh::LibvirtSshOpts {
            domain_name: vm_name.clone(),
            user: "root".to_string(),
            command: vec!["bootc".to_string(), "upgrade".to_string()],
            strict_host_keys: false,
            timeout: 600, // 10 minutes for upgrade
            log_level: "ERROR".to_string(),
            extra_options: vec![],
        };

        // Run the upgrade command and catch errors
        match libvirt::ssh::run(&libvirt_opts, upgrade_opts) {
            Ok(_) => {
                println!("Update staged successfully.");
            }
            Err(e) => {
                eprintln!("Error during bootc upgrade: {}", e);
                return Err(e);
            }
        }

        // Stage 2: Apply the update (will cause reboot)
        println!("Applying update and rebooting VM...");
        let apply_opts = libvirt::ssh::LibvirtSshOpts {
            domain_name: vm_name.clone(),
            user: "root".to_string(),
            command: vec![
                "bootc".to_string(),
                "upgrade".to_string(),
                "--apply".to_string(),
            ],
            strict_host_keys: false,
            timeout: 60,
            log_level: "ERROR".to_string(),
            extra_options: vec![],
        };

        // This will fail with connection error when VM reboots - that's expected
        let _ = libvirt::ssh::run(&libvirt_opts, apply_opts);
        println!("VM is rebooting to apply update...");

        // Wait for VM to come back up
        println!("Waiting for VM to restart...");
        std::thread::sleep(std::time::Duration::from_secs(5));

        // Wait for SSH to be available again
        let mut retries = 30;
        loop {
            let test_opts = libvirt::ssh::LibvirtSshOpts {
                domain_name: vm_name.clone(),
                user: "root".to_string(),
                command: vec!["true".to_string()],
                strict_host_keys: false,
                timeout: 5,
                log_level: "ERROR".to_string(),
                extra_options: vec![],
            };

            if libvirt::ssh::run(&libvirt_opts, test_opts).is_ok() {
                println!("VM is back online after update.");
                break;
            }

            retries -= 1;
            if retries == 0 {
                return Err(color_eyre::eyre::eyre!(
                    "Timeout waiting for VM to come back online after update"
                ));
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }

    // SSH into the VM (interactive shell or command execution)
    let ssh_opts = libvirt::ssh::LibvirtSshOpts {
        domain_name: vm_name.clone(),
        user: "root".to_string(),
        command: opts.command,
        strict_host_keys: false,
        timeout: 30,
        log_level: "ERROR".to_string(),
        extra_options: vec![],
    };

    libvirt::ssh::run(&libvirt_opts, ssh_opts)
}

/// Ensure the VM is running, starting it if necessary
fn ensure_vm_running(name: &str, libvirt_opts: &LibvirtOptions) -> Result<()> {
    use crate::domain_list::DomainLister;

    let lister = if let Some(ref uri) = libvirt_opts.connect {
        DomainLister::with_connection(uri.clone())
    } else {
        DomainLister::new()
    };
    let domains = lister.list_bootc_domains()?;

    let domain = domains.iter().find(|d| d.name == name).ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "Project VM '{}' not found. Run 'bcvk project up' first.",
            name
        )
    })?;

    match domain.state.as_str() {
        "running" => {
            // Already running, nothing to do
            Ok(())
        }
        "shut off" | "paused" => {
            println!("Starting project VM '{}'...", name);
            let start_opts = libvirt::start::LibvirtStartOpts {
                name: name.to_string(),
                ssh: false,
            };
            libvirt::start::run(libvirt_opts, start_opts)
        }
        state => {
            color_eyre::eyre::bail!(
                "Project VM '{}' is in unexpected state '{}'. \
                 Please check the VM status manually.",
                name,
                state
            );
        }
    }
}
