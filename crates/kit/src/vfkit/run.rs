//! vm run — Start a persistent VM from a container image or disk image.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Parser;
use color_eyre::{eyre::bail, Result};
use tracing::info;

use super::VmMetadata;
use crate::run_ephemeral_macos::{
    clear_xattr, expose_port, find_available_ssh_port, find_vfkit, generate_mac, start_gvproxy,
};
use crate::vm_helpers::{
    detect_machine_name, ensure_image_and_get_digest, parse_memory_to_mb, remove_file_if_exists,
    run_ssh_interactive, sanitize_vm_name, wait_for_ssh,
};

/// Port mapping from host to VM (format: host_port:guest_port).
#[derive(Debug, Clone)]
pub struct PortMapping {
    /// Host-side port number.
    pub host_port: u16,
    /// Guest-side port number.
    pub guest_port: u16,
}

impl std::str::FromStr for PortMapping {
    type Err = color_eyre::Report;
    fn from_str(s: &str) -> Result<Self> {
        let (host_part, guest_part) = s.split_once(':').ok_or_else(|| {
            color_eyre::eyre::eyre!(
                "Invalid port format '{}'. Expected format: host_port:guest_port",
                s
            )
        })?;
        let host_port = host_part
            .trim()
            .parse::<u16>()
            .map_err(|_| color_eyre::eyre::eyre!("Invalid host port '{}'", host_part))?;
        let guest_port = guest_part
            .trim()
            .parse::<u16>()
            .map_err(|_| color_eyre::eyre::eyre!("Invalid guest port '{}'", guest_part))?;
        Ok(PortMapping {
            host_port,
            guest_port,
        })
    }
}

/// Options for `vm run`.
#[derive(Parser, Debug)]
pub struct VmRunOpts {
    /// Container image or disk image path (.raw)
    #[clap(default_value = "")]
    pub image_or_disk: String,
    /// VM name (default: derived from image or disk filename)
    #[clap(long, short)]
    pub name: Option<String>,
    /// Instance type (e.g., u1.nano, u1.small). Overrides vcpus/memory if specified.
    #[clap(long)]
    pub itype: Option<crate::instancetypes::InstanceType>,
    /// Number of vCPUs (overridden by --itype if specified)
    #[clap(long)]
    pub vcpus: Option<u32>,
    /// Memory size (overridden by --itype if specified)
    #[clap(long, default_value = "4G")]
    pub memory: String,
    /// Path to an existing SSH private key
    #[clap(long)]
    pub ssh_key: Option<String>,
    /// SSH port (default: auto-allocate)
    #[clap(long)]
    pub ssh_port: Option<u16>,
    /// Display VM console in GUI window
    #[clap(long)]
    pub gui: bool,
    /// Disk size for to-disk (e.g. "10G", "20G")
    #[clap(long, default_value = "20G")]
    pub disk_size: String,
    /// Installation options (filesystem, root-size, etc.)
    #[clap(flatten)]
    pub install: crate::install_options::InstallOptions,
    /// Replace existing VM with same name
    #[clap(long, short = 'R')]
    pub replace: bool,
    /// Port mapping from host to VM (format: host_port:guest_port, e.g. 8080:80)
    #[clap(long = "port", short = 'p', action = clap::ArgAction::Append)]
    pub port_mappings: Vec<PortMapping>,
    /// User-defined labels for organizing VMs (comma not allowed in labels)
    #[clap(long)]
    pub label: Vec<String>,
    /// Automatically SSH into the VM after creation
    #[clap(long)]
    pub ssh: bool,
    /// Wait for SSH to become available and verify connectivity (for testing)
    #[clap(long, conflicts_with = "ssh")]
    pub ssh_wait: bool,
    /// Keep the VM running in background after creation (always true for vfkit)
    #[clap(long, short = 'd')]
    pub detach: bool,
}

fn validate_labels(labels: &[String]) -> Result<()> {
    for label in labels {
        if label.contains(',') {
            bail!("Label '{}' contains comma which is not allowed", label);
        }
    }
    Ok(())
}

fn is_disk_path(input: &str) -> bool {
    let p = Path::new(input);
    p.extension()
        .map(|e| e == "raw" || e == "img" || e == "qcow2")
        .unwrap_or(false)
        || p.exists()
}

/// Create and launch a persistent VM.
pub fn run(opts: VmRunOpts) -> Result<()> {
    if opts.image_or_disk.is_empty() {
        bail!("container image or disk path required");
    }
    validate_labels(&opts.label)?;

    let (disk_path_str, image_name) = if is_disk_path(&opts.image_or_disk) {
        let p = Path::new(&opts.image_or_disk);
        if !p.exists() {
            bail!("disk image not found: {}", opts.image_or_disk);
        }
        (opts.image_or_disk.clone(), None)
    } else {
        let image = &opts.image_or_disk;
        let vm_name = opts.name.clone().unwrap_or_else(|| sanitize_vm_name(image));

        if vm_name.is_empty() {
            bail!("could not derive VM name from image. Use --name to specify one.");
        }

        // Check existing VM
        if let Ok(existing) = VmMetadata::load(&vm_name) {
            if opts.replace {
                info!("replacing existing VM '{}'", vm_name);
                if existing.is_alive() {
                    if let Some(pid) = rustix::process::Pid::from_raw(existing.vfkit_pid as i32) {
                        if let Err(e) =
                            rustix::process::kill_process(pid, rustix::process::Signal::KILL)
                        {
                            tracing::warn!(
                                "failed to kill vfkit (pid {}): {}",
                                existing.vfkit_pid,
                                e
                            );
                        }
                    }
                    if let Some(pid) = rustix::process::Pid::from_raw(existing.gvproxy_pid as i32) {
                        if let Err(e) =
                            rustix::process::kill_process(pid, rustix::process::Signal::KILL)
                        {
                            tracing::warn!(
                                "failed to kill gvproxy (pid {}): {}",
                                existing.gvproxy_pid,
                                e
                            );
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                VmMetadata::remove(&vm_name);
            } else {
                bail!(
                    "VM '{}' already exists. Use --replace to overwrite, or --name to choose a different name.",
                    vm_name
                );
            }
        }

        let vms_dir = crate::to_disk_macos::vms_dir();
        fs::create_dir_all(&vms_dir)?;
        let disk_path = vms_dir.join(format!("{}.raw", vm_name));
        let key_path = PathBuf::from(format!("{}.key", disk_path.display()));
        let key_pub_path = PathBuf::from(format!("{}.pub", key_path.display()));

        if opts.replace {
            remove_file_if_exists(&disk_path);
            remove_file_if_exists(&key_path);
            remove_file_if_exists(&key_pub_path);
        }

        if !disk_path.exists() {
            info!("creating disk image for VM '{}'...", vm_name);
            let machine = detect_machine_name()?;
            let digest = ensure_image_and_get_digest(image)?;

            let base_disk_path = crate::to_disk_macos::find_or_create_base_disk(
                image,
                &digest,
                &opts.install,
                &opts.disk_size,
                &machine,
                &None,
                &[],
            )?;

            crate::to_disk_macos::clone_base_disk(&base_disk_path, &disk_path)?;

            let base_key = PathBuf::from(format!("{}.key", base_disk_path.display()));
            if base_key.exists() {
                fs::copy(&base_key, &key_path)?;
                let base_pub = PathBuf::from(format!("{}.pub", base_key.display()));
                if base_pub.exists() {
                    fs::copy(&base_pub, &key_pub_path)?;
                }
            }
        }

        (
            disk_path.to_string_lossy().to_string(),
            Some(image.to_string()),
        )
    };

    clear_xattr(Path::new(&disk_path_str));

    let ssh_key_path = find_ssh_key(&opts.ssh_key, &disk_path_str)?;

    let vm_name = opts.name.clone().unwrap_or_else(|| {
        Path::new(&disk_path_str)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("vm")
            .to_string()
    });

    let vms_dir = VmMetadata::vms_dir();
    fs::create_dir_all(&vms_dir)?;

    let efi_store = vms_dir.join(format!("{}-efi-vars", vm_name));
    let serial_log = vms_dir.join(format!("{}-serial.log", vm_name));
    let gvproxy_sock = vms_dir.join(format!("{}-gvproxy.sock", vm_name));
    let services_sock = vms_dir.join(format!("{}-gvproxy-svc.sock", vm_name));

    let gvproxy_sock_str = gvproxy_sock.to_string_lossy().to_string();
    let services_sock_str = services_sock.to_string_lossy().to_string();

    info!("starting gvproxy...");
    let gvproxy_child = start_gvproxy(&gvproxy_sock_str, &services_sock_str)?;

    let mac = generate_mac();
    let mac_str = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    let vcpus = opts.itype.map(|t| t.vcpus()).or(opts.vcpus).unwrap_or(2);
    let memory_mb = opts
        .itype
        .map(|t| t.memory_mb())
        .map(Ok)
        .unwrap_or_else(|| parse_memory_to_mb(&opts.memory))?;

    let vfkit_bin = find_vfkit()?;
    let mut vfkit_args = vec![
        "--cpus".to_string(),
        vcpus.to_string(),
        "--memory".to_string(),
        memory_mb.to_string(),
        "--bootloader".to_string(),
        format!("efi,variable-store={},create", efi_store.display()),
        "--device".to_string(),
        format!("virtio-blk,path={}", disk_path_str),
        "--device".to_string(),
        format!(
            "virtio-net,unixSocketPath={},mac={}",
            gvproxy_sock_str, mac_str
        ),
        "--device".to_string(),
        format!("virtio-serial,logFilePath={}", serial_log.display()),
        "--device".to_string(),
        "virtio-rng".to_string(),
    ];
    if opts.gui {
        vfkit_args.push("--gui".to_string());
    }

    info!("launching vfkit (EFI boot)...");
    let vfkit_child = Command::new(&vfkit_bin)
        .args(&vfkit_args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let ssh_port = opts.ssh_port.unwrap_or_else(find_available_ssh_port);
    info!("SSH port: {}", ssh_port);

    info!("setting up SSH port forwarding...");
    crate::utils::wait_for_readiness(
        indicatif::ProgressBar::hidden(),
        "Setting up SSH port forwarding",
        || match expose_port(
            &services_sock_str,
            crate::vm_helpers::GVPROXY_VM_IP,
            ssh_port,
            22,
        ) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        },
        std::time::Duration::from_secs(15),
        std::time::Duration::from_millis(500),
    )?;
    info!("SSH port {} forwarded", ssh_port);

    for pm in &opts.port_mappings {
        expose_port(
            &services_sock_str,
            crate::vm_helpers::GVPROXY_VM_IP,
            pm.host_port,
            pm.guest_port,
        )?;
        info!("port {}:{} forwarded", pm.host_port, pm.guest_port);
    }

    let key_path = std::path::Path::new(&ssh_key_path);
    wait_for_ssh(ssh_port, key_path, "root")?;

    let metadata = VmMetadata {
        name: vm_name.clone(),
        image: image_name,
        disk_image: disk_path_str.clone(),
        vfkit_pid: vfkit_child.id(),
        gvproxy_pid: gvproxy_child.id(),
        ssh_port,
        ssh_key: ssh_key_path.clone(),
        vcpus,
        memory_mb,
        efi_store: efi_store.to_string_lossy().to_string(),
        serial_log: serial_log.to_string_lossy().to_string(),
        gui: opts.gui,
        created: chrono::Utc::now().to_rfc3339(),
        state: "running".to_string(),
        labels: opts.label.clone(),
        port_mappings: opts
            .port_mappings
            .iter()
            .map(|pm| (pm.host_port, pm.guest_port))
            .collect(),
    };
    metadata.save()?;

    println!("VM '{}' is running", vm_name);
    println!(
        "  ssh -p {} -i {} {}@localhost",
        ssh_port, ssh_key_path, "root"
    );
    println!();
    println!("To connect:  bcvk vm ssh {}", vm_name);
    println!("To stop:     bcvk vm stop {}", vm_name);

    if opts.ssh_wait {
        println!("Ready; use bcvk vm ssh to connect");
        return Ok(());
    }
    if opts.ssh {
        let status = run_ssh_interactive(ssh_port, key_path, "root")?;
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

fn find_ssh_key(explicit: &Option<String>, disk_path: &str) -> Result<String> {
    if let Some(p) = explicit {
        if !Path::new(p).exists() {
            bail!("SSH key not found: {}", p);
        }
        return Ok(p.clone());
    }
    let auto_key = format!("{}.key", disk_path);
    if Path::new(&auto_key).exists() {
        info!("using auto-generated SSH key: {}", auto_key);
        return Ok(auto_key);
    }
    let home = dirs::home_dir()
        .ok_or_else(|| color_eyre::eyre::eyre!("cannot determine home directory"))?;
    for name in &["id_ed25519", "id_rsa"] {
        let path = home.join(".ssh").join(name);
        if path.exists() {
            return Ok(path.to_string_lossy().to_string());
        }
    }
    bail!("no SSH key found. Specify with --ssh-key")
}
