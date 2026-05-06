//! vm start — Restart a stopped persistent VM.

use std::process::{Command, Stdio};

use clap::Parser;
use color_eyre::{eyre::bail, Result};
use tracing::info;

use super::VmMetadata;
use crate::run_ephemeral_macos::{
    clear_xattr, expose_ssh_port, find_vfkit, generate_mac, start_gvproxy, wait_for_ssh,
};

/// Options for `vm start`.
#[derive(Parser, Debug)]
pub struct VmStartOpts {
    /// VM name
    pub name: String,
    /// Display VM console in GUI window
    #[clap(long)]
    pub gui: bool,
}

/// Restart a stopped persistent VM by re-launching vfkit.
pub fn run(opts: VmStartOpts) -> Result<()> {
    let mut meta = VmMetadata::load(&opts.name)?;
    if meta.is_alive() {
        bail!("VM '{}' is already running", opts.name);
    }

    if !std::path::Path::new(&meta.disk_image).exists() {
        bail!("disk image not found: {}", meta.disk_image);
    }
    clear_xattr(std::path::Path::new(&meta.disk_image));

    let vfkit_bin = find_vfkit()?;
    let vms_dir = VmMetadata::vms_dir();

    let gvproxy_sock = vms_dir.join(format!("{}-gvproxy.sock", meta.name));
    let services_sock = vms_dir.join(format!("{}-gvproxy-svc.sock", meta.name));
    let gvproxy_sock_str = gvproxy_sock.to_string_lossy().to_string();
    let services_sock_str = services_sock.to_string_lossy().to_string();

    info!("starting gvproxy...");
    let gvproxy_child = start_gvproxy(&gvproxy_sock_str, &services_sock_str)?;

    let mac = generate_mac();
    let mac_str = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    let gui = opts.gui || meta.gui;
    let mut vfkit_args = vec![
        "--cpus".to_string(),
        meta.cpus.to_string(),
        "--memory".to_string(),
        meta.memory.to_string(),
        "--bootloader".to_string(),
        format!("efi,variable-store={},create", meta.efi_store),
        "--device".to_string(),
        format!("virtio-blk,path={}", meta.disk_image),
        "--device".to_string(),
        format!(
            "virtio-net,unixSocketPath={},mac={}",
            gvproxy_sock_str, mac_str
        ),
        "--device".to_string(),
        format!("virtio-serial,logFilePath={}", meta.serial_log),
        "--device".to_string(),
        "virtio-rng".to_string(),
    ];
    if gui {
        vfkit_args.push("--gui".to_string());
    }

    info!("launching vfkit (EFI boot)...");
    let vfkit_child = Command::new(&vfkit_bin)
        .args(&vfkit_args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    info!("setting up SSH port forwarding...");
    for attempt in 0..15u32 {
        match expose_ssh_port(&services_sock_str, "192.168.127.2", meta.ssh_port) {
            Ok(_) => {
                info!("SSH port {} forwarded", meta.ssh_port);
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

    let key_path = std::path::Path::new(&meta.ssh_key);
    wait_for_ssh(meta.ssh_port, key_path, &meta.ssh_user)?;

    meta.vfkit_pid = vfkit_child.id();
    meta.gvproxy_pid = gvproxy_child.id();
    meta.state = "running".to_string();
    meta.gui = gui;
    meta.save()?;

    println!("Started '{}'", meta.name);
    println!(
        "  ssh -p {} -i {} {}@localhost",
        meta.ssh_port, meta.ssh_key, meta.ssh_user
    );
    Ok(())
}
