//! vm run — Start a persistent VM from a disk image using vfkit + EFI boot.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use clap::Parser;
use color_eyre::{eyre::bail, Result};
use tracing::info;

use super::VmMetadata;
use crate::run_ephemeral_macos::{
    clear_xattr, expose_ssh_port, find_available_ssh_port, find_vfkit, generate_mac, start_gvproxy,
    wait_for_ssh,
};

/// Options for `vm run`.
#[derive(Parser, Debug)]
pub struct VmRunOpts {
    /// Disk image path (.raw)
    pub disk: String,
    /// VM name for identification
    #[clap(long)]
    pub name: Option<String>,
    /// Number of vCPUs
    #[clap(long)]
    pub vcpus: Option<u32>,
    /// Memory size (e.g. "4G", "2048M", or plain number for MB)
    #[clap(long, default_value = "4G")]
    pub memory: String,
    /// Path to an existing SSH private key
    #[clap(long)]
    pub ssh_key: Option<String>,
    /// SSH username (default: root)
    #[clap(long, default_value = "root")]
    pub ssh_user: String,
    /// SSH port (default: auto-allocate)
    #[clap(long)]
    pub ssh_port: Option<u16>,
    /// Display VM console in GUI window
    #[clap(long)]
    pub gui: bool,
}

/// Create and launch a persistent VM from a disk image via vfkit + EFI.
pub fn run(opts: VmRunOpts) -> Result<()> {
    let vfkit_bin = find_vfkit()?;

    if !Path::new(&opts.disk).exists() {
        bail!("disk image not found: {}", opts.disk);
    }
    clear_xattr(Path::new(&opts.disk));

    let ssh_key_path = match &opts.ssh_key {
        Some(p) => p.clone(),
        None => find_ssh_key()?,
    };
    if !Path::new(&ssh_key_path).exists() {
        bail!(
            "SSH key not found: {}. Specify with --ssh-key",
            ssh_key_path
        );
    }

    let vm_name = opts.name.clone().unwrap_or_else(|| {
        Path::new(&opts.disk)
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

    let vcpus = opts.vcpus.unwrap_or(2);
    let memory_mb = crate::run_ephemeral_macos::parse_memory_to_mb(&opts.memory)?;

    let mut vfkit_args = vec![
        "--cpus".to_string(),
        vcpus.to_string(),
        "--memory".to_string(),
        memory_mb.to_string(),
        "--bootloader".to_string(),
        format!("efi,variable-store={},create", efi_store.display()),
        "--device".to_string(),
        format!("virtio-blk,path={}", opts.disk),
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
    for attempt in 0..15u32 {
        match expose_ssh_port(&services_sock_str, "192.168.127.2", ssh_port) {
            Ok(_) => {
                info!("SSH port {} forwarded", ssh_port);
                break;
            }
            Err(e) if attempt < 14 => {
                tracing::debug!("SSH port forward attempt {}: {}", attempt, e);
                let backoff = 200 * 2u64.pow(attempt.min(4));
                std::thread::sleep(std::time::Duration::from_millis(backoff));
            }
            Err(e) => bail!("SSH port forward failed: {}", e),
        }
    }

    let key_path = std::path::Path::new(&ssh_key_path);
    wait_for_ssh(ssh_port, key_path, &opts.ssh_user)?;

    let metadata = VmMetadata {
        name: vm_name.clone(),
        disk_image: opts.disk.clone(),
        vfkit_pid: vfkit_child.id(),
        gvproxy_pid: gvproxy_child.id(),
        ssh_port,
        ssh_key: ssh_key_path.clone(),
        ssh_user: opts.ssh_user.clone(),
        cpus: vcpus,
        memory: memory_mb,
        efi_store: efi_store.to_string_lossy().to_string(),
        serial_log: serial_log.to_string_lossy().to_string(),
        gui: opts.gui,
        created: chrono::Utc::now().to_rfc3339(),
        state: "running".to_string(),
    };
    metadata.save()?;

    println!("VM '{}' is running", vm_name);
    println!(
        "  ssh -p {} -i {} {}@localhost",
        ssh_port, ssh_key_path, opts.ssh_user
    );
    println!();
    println!("To connect:  bcvk vm ssh {}", vm_name);
    println!("To stop:     bcvk vm stop {}", vm_name);

    Ok(())
}

fn find_ssh_key() -> Result<String> {
    let home = dirs::home_dir()
        .ok_or_else(|| color_eyre::eyre::eyre!("cannot determine home directory"))?;
    for name in &["id_ed25519", "id_rsa"] {
        let path = home.join(".ssh").join(name);
        if path.exists() {
            return Ok(path.to_string_lossy().to_string());
        }
    }
    bail!("no SSH key found in ~/.ssh/. Generate with: ssh-keygen -t ed25519")
}
