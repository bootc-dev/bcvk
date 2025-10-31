//! Implementation of `bcvk project up` command

use camino::Utf8Path;
use clap::Parser;
use color_eyre::{eyre::Context as _, Result};

use crate::libvirt::{self, run::spawn_lifecycle_monitor, LibvirtOptions};

use super::{config::ProjectConfig, current_project_dir, project_vm_name};

/// Create or start the project VM
///
/// Automatically names and manages a VM scoped to the current project directory.
/// Won't recreate if a VM with the same name already exists.
#[derive(Debug, Parser)]
pub struct ProjectUpOpts {
    /// Libvirt connection URI (defaults to qemu:///session)
    #[clap(long)]
    pub connect: Option<String>,

    /// Automatically SSH into the VM after creation
    #[clap(long)]
    pub ssh: bool,

    /// Disable lifecycle binding (don't shutdown VM when parent exits)
    #[clap(long, short = 'L')]
    pub no_lifecycle_bind: bool,

    /// Enable automatic updates via bootc-fetch-apply-updates every 30s
    #[clap(long)]
    pub auto_update: bool,

    /// Reset: remove existing VM (force stop and delete) before creating new one
    #[clap(long, short = 'R')]
    pub reset: bool,
}

/// Run the project up command
pub fn run(opts: ProjectUpOpts) -> Result<()> {
    // Get current project directory
    let project_dir = current_project_dir()?;

    // Load project configuration
    let config = ProjectConfig::load_from_dir(&project_dir)?.ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "No .bcvk/config.toml found in current directory.\n\
                 Run 'bcvk project init' to create one interactively,\n\
                 or create .bcvk/config.toml manually with at least vm.image specified."
        )
    })?;

    // Generate project VM name
    let vm_name = project_vm_name(&project_dir, Some(&config))?;

    // Build libvirt options
    let libvirt_opts = LibvirtOptions {
        connect: opts.connect.clone(),
    };

    // If reset flag is set, remove existing VM first
    if opts.reset {
        let rm_opts = libvirt::rm::LibvirtRmOpts {
            name: vm_name.clone(),
            force: true,
            stop: true,
        };
        // Ignore errors from rm (VM might not exist)
        let _ = libvirt::rm::run(&libvirt_opts, rm_opts);
    }

    let existing_vm = check_vm_exists(&vm_name, &libvirt_opts)?;

    if let Some(state) = existing_vm {
        match state.as_str() {
            "running" => {
                println!("Project VM '{}' is already running", vm_name);
                // Spawn lifecycle monitor for already-running VM
                if !opts.no_lifecycle_bind {
                    spawn_lifecycle_monitor(&vm_name, libvirt_opts.connect.as_deref())?;
                }
                if opts.ssh {
                    ssh_into_vm(&vm_name, &libvirt_opts)?;
                }
                return Ok(());
            }
            "shut off" | "paused" => {
                println!("Starting existing project VM '{}'...", vm_name);
                start_vm(&vm_name, &libvirt_opts)?;
                // Spawn lifecycle monitor after starting VM
                if !opts.no_lifecycle_bind {
                    spawn_lifecycle_monitor(&vm_name, libvirt_opts.connect.as_deref())?;
                }
                if opts.ssh {
                    ssh_into_vm(&vm_name, &libvirt_opts)?;
                }
                return Ok(());
            }
            _ => {
                println!(
                    "Project VM '{}' exists in state '{}', starting...",
                    vm_name, state
                );
                start_vm(&vm_name, &libvirt_opts)?;
                // Spawn lifecycle monitor after starting VM
                if !opts.no_lifecycle_bind {
                    spawn_lifecycle_monitor(&vm_name, libvirt_opts.connect.as_deref())?;
                }
                if opts.ssh {
                    ssh_into_vm(&vm_name, &libvirt_opts)?;
                }
                return Ok(());
            }
        }
    }

    // VM doesn't exist, create it
    println!("Creating project VM '{}'...", vm_name);
    create_vm(
        &vm_name,
        &config,
        &project_dir,
        &libvirt_opts,
        opts.ssh,
        !opts.no_lifecycle_bind,
        opts.auto_update,
    )?;

    Ok(())
}

/// Check if a VM exists and return its state
fn check_vm_exists(name: &str, libvirt_opts: &LibvirtOptions) -> Result<Option<String>> {
    use crate::domain_list::DomainLister;

    let lister = if let Some(ref uri) = libvirt_opts.connect {
        DomainLister::with_connection(uri.clone())
    } else {
        DomainLister::new()
    };
    let domains = lister.list_bootc_domains()?;

    for domain in domains {
        if domain.name == name {
            return Ok(Some(domain.state));
        }
    }

    Ok(None)
}

/// Start an existing VM
fn start_vm(name: &str, libvirt_opts: &LibvirtOptions) -> Result<()> {
    let start_opts = libvirt::start::LibvirtStartOpts {
        name: name.to_string(),
        ssh: false,
    };

    libvirt::start::run(libvirt_opts, start_opts)
}

/// SSH into a running VM
fn ssh_into_vm(name: &str, libvirt_opts: &LibvirtOptions) -> Result<()> {
    let ssh_opts = libvirt::ssh::LibvirtSshOpts {
        domain_name: name.to_string(),
        user: "root".to_string(),
        command: vec![],
        strict_host_keys: false,
        timeout: 30,
        log_level: "ERROR".to_string(),
        extra_options: vec![],
    };

    libvirt::ssh::run(libvirt_opts, ssh_opts)
}

/// Create a new project VM
fn create_vm(
    name: &str,
    config: &ProjectConfig,
    project_dir: &Utf8Path,
    libvirt_opts: &LibvirtOptions,
    ssh: bool,
    lifecycle_bind: bool,
    auto_update: bool,
) -> Result<()> {
    use crate::install_options::InstallOptions;
    use crate::libvirt::run::{FirmwareType, LibvirtRunOpts};

    // Build run options from project config
    // We know vm exists because load_from_dir validates it
    let vm = config.vm.as_ref().unwrap();

    let mut run_opts = LibvirtRunOpts {
        image: vm.image.clone(),
        name: Some(name.to_string()),
        memory: vm.memory.clone(),
        cpus: vm.cpu.cpus,
        disk_size: vm.disk.disk_size.clone(),
        install: InstallOptions::default(),
        port_mappings: vec![],
        raw_volumes: vec![],
        bind_mounts: vec![],
        bind_mounts_ro: vec![],
        network: vm.net.network.clone(),
        detach: !ssh, // Don't detach if we're going to SSH
        ssh,
        bind_storage_ro: true,
        firmware: FirmwareType::UefiSecure,
        disable_tpm: false,
        secure_boot_keys: None,
        label: vec!["bcvk-project".to_string()],
        transient: false,
        lifecycle_bind_parent: lifecycle_bind,
        metadata: {
            let mut m = std::collections::HashMap::new();
            m.insert("bootc:project-dir".to_string(), project_dir.to_string());
            m
        },
        extra_smbios_credentials: vec![],
    };

    run_opts.install.filesystem = Some(
        vm.filesystem
            .clone()
            .unwrap_or_else(|| crate::libvirt::LIBVIRT_DEFAULT_FILESYSTEM.to_string()),
    );

    // Bind project directory to /run/src read-only with auto-mount
    // (will fall back to read-write if libvirt doesn't support readonly virtiofs)
    run_opts.bind_mounts_ro.push(
        format!("{}:/run/src", project_dir.as_str())
            .parse()
            .context("Failed to parse project directory bind mount")?,
    );

    // Add configured mounts using bind mount options
    for mount in config.mounts.iter().flatten() {
        let mount_spec = format!("{}/{}:{}", project_dir.as_str(), mount.host, mount.guest);
        let bind_mount = mount_spec
            .parse()
            .with_context(|| format!("Failed to parse mount spec: {}", mount_spec))?;
        if mount.readonly {
            run_opts.bind_mounts_ro.push(bind_mount);
        } else {
            run_opts.bind_mounts.push(bind_mount);
        }
    }

    // Check for systemd units
    let units_dir = config
        .systemd_units_dir(project_dir)
        .or_else(|| ProjectConfig::default_units_dir(project_dir));

    if let Some(units_dir) = units_dir {
        if units_dir.exists() {
            println!("Injecting systemd units from: {}", units_dir);
            // TODO: Implement systemd unit injection
            // For now, warn that it's not yet implemented
            eprintln!(
                "Warning: Systemd unit injection is not yet implemented. Units in {} will be ignored.",
                units_dir
            );
        }
    }

    // Configure auto-update if requested
    if auto_update {
        println!("Enabling automatic updates (every 30s)...");

        // Generate dropin for bootc-fetch-apply-updates.service to use host container storage
        let service_dropin_content = "\
[Service]
Environment=STORAGE_OPTS=additionalimagestore=/run/host-container-storage
";
        let service_dropin_encoded =
            data_encoding::BASE64.encode(service_dropin_content.as_bytes());
        let service_dropin_cred = format!(
            "io.systemd.credential.binary:systemd.unit-dropin.bootc-fetch-apply-updates.service~bcvk-auto-update={}",
            service_dropin_encoded
        );
        run_opts.extra_smbios_credentials.push(service_dropin_cred);

        // Generate dropin for bootc-fetch-apply-updates.timer to run every 30s
        let timer_dropin_content = "\
[Timer]
OnBootSec=
OnCalendar=
OnUnitActiveSec=
OnUnitInactiveSec=
OnBootSec=30
OnUnitActiveSec=30
";
        let timer_dropin_encoded = data_encoding::BASE64.encode(timer_dropin_content.as_bytes());
        let timer_dropin_cred = format!(
            "io.systemd.credential.binary:systemd.unit-dropin.bootc-fetch-apply-updates.timer~bcvk-auto-update={}",
            timer_dropin_encoded
        );
        run_opts.extra_smbios_credentials.push(timer_dropin_cred);
    }

    // Note: STORAGE_OPTS environment configuration is injected in libvirt::run::run() via:
    // - systemd.extra-unit (bcvk-storage-opts.service) for /etc/environment (PAM/SSH sessions)
    // - tmpfiles.extra for systemd user/system manager configuration

    // Run the VM
    libvirt::run::run(libvirt_opts, run_opts)
        .with_context(|| format!("Failed to create project VM '{}'", name))?;

    Ok(())
}
