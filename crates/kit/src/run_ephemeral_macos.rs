//! Ephemeral VM launch flow for macOS using vfkit + NBD EROFS over TCP.
//!
//! Boot flow (fully diskless):
//! 1. Mount container image overlay (`podman image mount`)
//! 2. Start bcvk-nbd server in TCP mode (port forwarded via gvproxy)
//! 3. Launch vfkit with EFI boot via NBD TCP + virtio-net (gvproxy)
//! 4. Wait for SSH and execute commands
//!
//! Common helpers (gvproxy, SSH, vfkit detection) are pub for reuse by vfkit/ module.

use std::fs;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use color_eyre::{
    eyre::{bail, Context},
    Result,
};
use tracing::{debug, info};

pub use crate::vm_helpers::{
    default_vcpus, detect_machine_name, ensure_image_and_get_digest, is_machine_rootful,
    parse_memory_to_mb, run_ssh_command, run_ssh_interactive, wait_for_ssh,
};

/// Base directory for ephemeral VM state on macOS host.
pub fn ephemeral_base_dir() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(".local/share/bcvk/ephemeral")
}

// --- Data structures ---

/// Metadata for a running ephemeral VM, persisted as JSON for `ps` and `ssh`.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct EphemeralVmMetadata {
    /// VM name used as identifier for resource isolation.
    pub name: String,
    /// Container image reference used to boot the VM.
    pub image: String,
    /// PID of the vfkit process.
    pub pid: u32,
    /// PID of the gvproxy network proxy process.
    pub gvproxy_pid: u32,
    /// Host-side SSH port forwarded to the VM.
    pub ssh_port: u16,
    /// Path to the SSH private key for this VM.
    pub ssh_key: String,
    /// Path to the serial console log file.
    pub serial_log: String,
    /// Path to the vfkit process log file.
    pub log_path: Option<String>,
    /// ISO 8601 timestamp when the VM was created.
    pub created: String,
    /// Systemd unit name of the bcvk-nbd server serving the rootfs.
    #[serde(default)]
    pub nbd_unit: Option<String>,
    /// NBD port allocated for this VM's rootfs.
    #[serde(default)]
    pub nbd_port: Option<u16>,
}

impl EphemeralVmMetadata {
    /// Return the directory path for ephemeral VM metadata files.
    pub fn vms_dir() -> std::path::PathBuf {
        ephemeral_base_dir().join("vms")
    }

    /// Save metadata to a JSON file in the VMs directory.
    pub fn save(&self) -> Result<()> {
        let dir = Self::vms_dir();
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", self.name));
        fs::write(&path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Remove metadata file for the named VM.
    pub fn remove(name: &str) {
        let path = Self::vms_dir().join(format!("{}.json", name));
        crate::vm_helpers::remove_file_if_exists(&path);
    }

    /// Load metadata for the named VM from its JSON file.
    pub fn load(name: &str) -> Result<Self> {
        let path = Self::vms_dir().join(format!("{}.json", name));
        let data = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data)?)
    }

    /// List all ephemeral VM metadata from the VMs directory.
    pub fn list_all() -> Result<Vec<Self>> {
        let dir = Self::vms_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut vms = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(meta) = serde_json::from_str::<Self>(&data) {
                    vms.push(meta);
                }
            }
        }
        Ok(vms)
    }

    /// Check if the VM process is still alive via kill(pid, 0).
    pub fn is_alive(&self) -> bool {
        rustix::process::Pid::from_raw(self.pid as i32)
            .map(|pid| rustix::process::test_kill_process(pid).is_ok())
            .unwrap_or(false)
    }
}

/// Options for launching an ephemeral VM via vfkit.
#[derive(clap::Parser, Debug)]
pub struct RunEphemeralOpts {
    /// Container image to boot
    pub image: String,
    /// Instance type (e.g., u1.nano, u1.small). Overrides vcpus/memory if specified.
    #[clap(long)]
    pub itype: Option<crate::instancetypes::InstanceType>,
    /// Number of vCPUs (overridden by --itype if specified)
    #[clap(long)]
    pub vcpus: Option<u32>,
    /// Memory size (overridden by --itype if specified)
    #[clap(long, default_value = "4G")]
    pub memory: String,
    /// Generate a temporary SSH key pair for VM access
    #[clap(long = "ssh-keygen", short = 'K')]
    pub ssh_keygen: bool,
    /// Command(s) to execute via SSH after boot
    #[clap(long)]
    pub execute: Vec<String>,
    /// VM name for identification and resource isolation
    #[clap(long)]
    pub name: Option<String>,
    /// Additional kernel command line arguments
    #[clap(long = "karg")]
    pub kernel_args: Vec<String>,
    /// Display VM console in GUI window
    #[clap(long)]
    pub gui: bool,
    /// Run in background
    #[clap(long, short = 'd')]
    pub detach: bool,
    /// Enable debug mode (reserved for future use)
    #[clap(long)]
    pub debug: bool,
}

// --- RAII cleanup guard ---

struct VmCleanup {
    vfkit_pid: u32,
    gvproxy_pid: u32,
    nbd_unit: Option<String>,
    nbd_port: Option<u16>,
    image: String,
    vm_name: String,
}

impl Drop for VmCleanup {
    fn drop(&mut self) {
        tracing::debug!("cleaning up VM processes...");
        if let Some(ref name) = self.nbd_unit {
            crate::nbd_macos::stop_nbd_server(name, self.nbd_port);
        }
        if let Some(pid) = rustix::process::Pid::from_raw(self.vfkit_pid as i32) {
            if let Err(e) = rustix::process::kill_process(pid, rustix::process::Signal::TERM) {
                tracing::warn!("failed to kill vfkit (PID {}): {}", self.vfkit_pid, e);
            }
        }
        if let Some(pid) = rustix::process::Pid::from_raw(self.gvproxy_pid as i32) {
            if let Err(e) = rustix::process::kill_process(pid, rustix::process::Signal::TERM) {
                tracing::warn!("failed to kill gvproxy (PID {}): {}", self.gvproxy_pid, e);
            }
        }
        // Release container image overlay mount
        if let Ok(machine) = detect_machine_name() {
            if let Err(e) = Command::new("podman")
                .args([
                    "machine",
                    "ssh",
                    &machine,
                    "--",
                    "podman",
                    "image",
                    "umount",
                    &self.image,
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
            {
                tracing::debug!("failed to umount image {}: {}", self.image, e);
            }
        }
        EphemeralVmMetadata::remove(&self.vm_name);
    }
}

// --- Main entry point ---

/// Run an ephemeral VM from a container image.
///
pub fn run(opts: RunEphemeralOpts) -> Result<()> {
    if opts.gui && opts.detach {
        bail!("--gui and --detach cannot be used together (GUI requires foreground process)");
    }
    run_vfkit(opts)
}

/// Run an ephemeral VM using vfkit + EROFS over NBD (TCP transport).
fn run_vfkit(opts: RunEphemeralOpts) -> Result<()> {
    if opts.detach {
        return run_detached(&opts);
    }

    let vfkit_bin = find_vfkit()?;
    info!(image = %opts.image, "starting ephemeral VM on macOS (vfkit + NBD TCP)");

    let cache_base = ephemeral_base_dir();
    fs::create_dir_all(&cache_base)?;

    let machine = detect_machine_name()?;
    let rootful = is_machine_rootful(&machine);
    debug!(
        "podman machine '{}' ({})",
        machine,
        if rootful { "rootful" } else { "rootless" }
    );
    let digest = ensure_image_and_get_digest(&opts.image)?;
    let digest_short = &digest[..16.min(digest.len())];
    info!("image digest: {}...", digest_short);

    let vm_name = opts
        .name
        .clone()
        .unwrap_or_else(|| format!("ephemeral-{}", &digest_short[..8]));
    let ssh_key_path = cache_base.join(format!("{}-key", vm_name));

    let mut ssh_pubkey = String::new();
    if opts.ssh_keygen || !opts.execute.is_empty() {
        info!("generating SSH keypair...");
        ssh_pubkey = crate::vm_helpers::generate_ssh_keypair(&ssh_key_path)?;
    }

    let mut cmdline_parts: Vec<&str> = Vec::from(crate::kernel_cmdline::BASE_KERNEL_CMDLINE);
    cmdline_parts.extend([
        "root=/dev/vda2",
        "ro",
        "rootfstype=erofs",
        "console=tty0",
        "loglevel=4",
        "net.ifnames=0",
    ]);
    let user_args: Vec<&str> = opts.kernel_args.iter().map(|s| s.as_str()).collect();
    cmdline_parts.extend(&user_args);
    let cmdline = cmdline_parts.join(" ");

    // Deploy NBD server binary to podman machine (hash-checked, idempotent)
    crate::nbd_macos::deploy_nbd_server(&machine)?;

    // Get container image merged overlay path
    let merged_path = crate::nbd_macos::get_merged_path(&machine, rootful, &opts.image)?;
    info!("overlay merged: {}", merged_path);

    let nbd_port = crate::nbd_macos::find_available_nbd_port();
    info!("NBD transport: TCP (port {})", nbd_port);
    let nbd_unit_name = crate::nbd_macos::start_nbd_server(
        &machine,
        &merged_path,
        &cmdline,
        &ssh_pubkey,
        nbd_port,
        &vm_name,
    )?;

    // gvproxy + vfkit (EFI boot)
    let gvproxy_sock = cache_base.join(format!("{}-gvproxy.sock", vm_name));
    let services_sock = cache_base.join(format!("{}-gvproxy-svc.sock", vm_name));
    let gvproxy_sock_str = gvproxy_sock.to_string_lossy().to_string();
    let services_sock_str = services_sock.to_string_lossy().to_string();
    info!("starting gvproxy...");
    let mut gvproxy_child = start_gvproxy(&gvproxy_sock_str, &services_sock_str)?;

    let mac = generate_mac();
    let mac_str = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    let efi_var_store = cache_base.join(format!("{}-efi-vars", vm_name));
    let bootloader_arg = format!("efi,variable-store={},create", efi_var_store.display());

    let vcpus = opts
        .itype
        .map(|t| t.vcpus())
        .or(opts.vcpus)
        .unwrap_or_else(default_vcpus);
    let memory_mb = opts
        .itype
        .map(|t| t.memory_mb())
        .map(Ok)
        .unwrap_or_else(|| parse_memory_to_mb(&opts.memory))?;

    let mut vfkit_args = vec![
        "--cpus".to_string(),
        vcpus.to_string(),
        "--memory".to_string(),
        memory_mb.to_string(),
        "--bootloader".to_string(),
        bootloader_arg,
        "--device".to_string(),
        format!(
            "nbd,uri=nbd://127.0.0.1:{}/,readonly,timeout=5000,deviceId=rootfs",
            nbd_port
        ),
        "--device".to_string(),
        format!(
            "virtio-net,unixSocketPath={},mac={}",
            gvproxy_sock_str, mac_str
        ),
        "--device".to_string(),
        "virtio-rng".to_string(),
        "--device".to_string(),
        format!(
            "virtio-vsock,port=9000,socketURL={},connect",
            cache_base.join(format!("{}-vsock.sock", vm_name)).display()
        ),
    ];

    let serial_log = cache_base.join(format!("{}-serial.log", vm_name));
    vfkit_args.extend([
        "--device".to_string(),
        format!("virtio-serial,logFilePath={}", serial_log.display()),
    ]);

    if opts.gui {
        vfkit_args.push("--gui".to_string());
    }

    info!("launching vfkit...");
    let vfkit_log = cache_base.join(format!("{}-vfkit.log", vm_name));
    let vfkit_log_file = fs::File::create(&vfkit_log)?;
    let mut vfkit_child = Command::new(&vfkit_bin)
        .args(&vfkit_args)
        .stdout(vfkit_log_file.try_clone()?)
        .stderr(vfkit_log_file)
        .spawn()
        .context("failed to start vfkit")?;

    let ssh_port = find_available_ssh_port();
    debug!("allocated SSH port: {}", ssh_port);

    let metadata = EphemeralVmMetadata {
        name: vm_name.clone(),
        image: opts.image.clone(),
        pid: vfkit_child.id(),
        gvproxy_pid: gvproxy_child.id(),
        ssh_port,
        ssh_key: ssh_key_path.to_string_lossy().to_string(),
        serial_log: serial_log.to_string_lossy().to_string(),
        log_path: None,
        created: chrono::Utc::now().to_rfc3339(),
        nbd_unit: Some(nbd_unit_name.clone()),
        nbd_port: Some(nbd_port),
    };
    metadata.save()?;

    let _cleanup = VmCleanup {
        vfkit_pid: vfkit_child.id(),
        gvproxy_pid: gvproxy_child.id(),
        nbd_unit: Some(nbd_unit_name.clone()),
        nbd_port: Some(nbd_port),
        image: opts.image.clone(),
        vm_name: vm_name.clone(),
    };

    if opts.ssh_keygen || !opts.execute.is_empty() {
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
            Duration::from_secs(15),
            Duration::from_millis(500),
        )?;
        info!("SSH port {} forwarded", ssh_port);

        wait_for_ssh(ssh_port, &ssh_key_path, "root")?;

        if !opts.execute.is_empty() {
            for cmd_str in &opts.execute {
                info!("executing: {}", cmd_str);
                let status = run_ssh_command(ssh_port, &ssh_key_path, "root", cmd_str)?;
                if !status.success() {
                    bail!("command failed: {}", status);
                }
            }
            return Ok(());
        }

        info!(
            "SSH ready: ssh -p {} -i {} root@localhost",
            ssh_port,
            ssh_key_path.display()
        );

        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            let status = run_ssh_interactive(ssh_port, &ssh_key_path, "root")?;
            let exit_code = status.code().unwrap_or(1);
            drop(_cleanup);
            std::process::exit(exit_code);
        }
    }

    // No SSH: wait for vfkit to exit (GUI window closed or VM shutdown)
    std::mem::forget(_cleanup);
    let status = vfkit_child.wait()?;
    info!("vfkit exited: {}", status);
    crate::nbd_macos::stop_nbd_server(&nbd_unit_name, Some(nbd_port));
    if let Err(e) = gvproxy_child.kill() {
        tracing::debug!("failed to kill gvproxy: {}", e);
    }
    // Release container image overlay mount
    if let Err(e) = Command::new("podman")
        .args([
            "machine",
            "ssh",
            &machine,
            "--",
            "podman",
            "image",
            "umount",
            &opts.image,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        tracing::debug!("failed to umount image {}: {}", opts.image, e);
    }
    EphemeralVmMetadata::remove(&vm_name);
    Ok(())
}

fn run_detached(opts: &RunEphemeralOpts) -> Result<()> {
    let cache_base = ephemeral_base_dir();
    fs::create_dir_all(&cache_base)?;
    let digest = ensure_image_and_get_digest(&opts.image)?;
    let digest_short = &digest[..16.min(digest.len())];
    let vm_name = opts
        .name
        .clone()
        .unwrap_or_else(|| format!("ephemeral-{}", &digest_short[..8]));
    let log_path = cache_base.join(format!("bcvk-{}.log", vm_name));
    let log_file = fs::File::create(&log_path)?;

    let exe = std::env::current_exe()?;
    let mut args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| a != "--detach" && a != "-d")
        .collect();
    if !args.contains(&"-K".to_string()) && !args.contains(&"--ssh-keygen".to_string()) {
        args.insert(args.len() - 1, "-K".to_string());
    }
    if opts.name.is_none() {
        args.insert(args.len() - 1, "--name".to_string());
        args.insert(args.len() - 1, vm_name.clone());
    }

    let child = Command::new(exe)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(log_file.try_clone()?)
        .stderr(log_file)
        .spawn()?;

    let metadata = EphemeralVmMetadata {
        name: vm_name.clone(),
        image: opts.image.clone(),
        pid: child.id(),
        gvproxy_pid: 0,
        ssh_port: 0,
        ssh_key: cache_base
            .join(format!("{}-key", vm_name))
            .to_string_lossy()
            .to_string(),
        serial_log: String::new(),
        log_path: Some(log_path.to_string_lossy().to_string()),
        created: chrono::Utc::now().to_rfc3339(),
        nbd_unit: None,
        nbd_port: None,
    };
    metadata.save()?;
    println!("{}", vm_name);
    Ok(())
}

// --- macOS-specific helpers (pub for vfkit/ module) ---

/// Clear extended attributes from a file.
///
/// Apple Virtualization.framework rejects disk images with xattrs like
/// `security.selinux` or `user.containers.override_stat` that are added
/// by podman/buildah when creating images inside containers.
pub fn clear_xattr(path: &Path) {
    if let Err(e) = Command::new("xattr")
        .args(["-c", &path.to_string_lossy()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        tracing::debug!("failed to clear xattr on {}: {}", path.display(), e);
    }
}

/// Find the vfkit binary, checking PATH and Podman PKG location.
pub fn find_vfkit() -> Result<String> {
    if let Ok(path) = which::which("vfkit") {
        return Ok(path.to_string_lossy().to_string());
    }
    let podman_path = "/opt/podman/bin/vfkit";
    if Path::new(podman_path).exists() {
        return Ok(podman_path.to_string());
    }
    bail!("vfkit not found. Install: brew install vfkit")
}

/// Fixed MAC address matching gvproxy's DHCP static lease for [`GVPROXY_VM_IP`](crate::vm_helpers::GVPROXY_VM_IP).
const GVPROXY_STATIC_MAC: [u8; 6] = [0x5a, 0x94, 0xef, 0xe4, 0x0c, 0xee];

/// Generate the fixed MAC address for gvproxy DHCP static lease.
pub fn generate_mac() -> [u8; 6] {
    GVPROXY_STATIC_MAC
}

/// Find the gvproxy binary, checking PATH and Podman installation paths.
fn find_gvproxy() -> Result<String> {
    if let Ok(path) = which::which("gvproxy") {
        return Ok(path.to_string_lossy().to_string());
    }
    for candidate in [
        "/opt/homebrew/opt/podman/libexec/podman/gvproxy",
        "/opt/podman/bin/gvproxy",
    ] {
        if Path::new(candidate).exists() {
            return Ok(candidate.to_string());
        }
    }
    bail!("gvproxy not found. Ensure Podman is installed (brew install podman)")
}

/// Start a gvproxy instance with the given socket paths.
pub fn start_gvproxy(gvproxy_sock: &str, services_sock: &str) -> Result<std::process::Child> {
    let gvproxy_bin = find_gvproxy()?;
    crate::vm_helpers::remove_file_if_exists(std::path::Path::new(gvproxy_sock));
    crate::vm_helpers::remove_file_if_exists(std::path::Path::new(services_sock));
    let child = Command::new(&gvproxy_bin)
        .args([
            "-listen-vfkit",
            &format!("unixgram://{}", gvproxy_sock),
            "-ssh-port",
            "-1",
            "-services",
            &format!("unix://{}", services_sock),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start gvproxy. Ensure gvproxy is installed (included in Podman)")?;
    crate::utils::wait_for_readiness(
        indicatif::ProgressBar::hidden(),
        "Waiting for gvproxy socket",
        || Ok(Path::new(gvproxy_sock).exists()),
        Duration::from_secs(5),
        Duration::from_millis(100),
    )?;
    Ok(child)
}

/// Expose a TCP port forwarding rule via gvproxy's HTTP API.
pub fn expose_port(
    services_sock: &str,
    vm_ip: &str,
    host_port: u16,
    guest_port: u16,
) -> Result<()> {
    let body = format!(
        r#"{{"local":":{}","remote":"{}:{}","protocol":"tcp"}}"#,
        host_port, vm_ip, guest_port
    );
    let mut stream = UnixStream::connect(services_sock)?;
    let request = format!(
        "POST /services/forwarder/expose HTTP/1.1\r\nHost: unix\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    std::io::Write::write_all(&mut stream, request.as_bytes())?;
    std::io::Write::flush(&mut stream)?;
    let mut response = vec![0u8; 1024];
    if let Err(e) = std::io::Read::read(&mut stream, &mut response) {
        tracing::debug!("failed to read gvproxy response: {}", e);
    }
    let response_str = String::from_utf8_lossy(&response);
    if !response_str.contains("200") {
        bail!(
            "gvproxy expose failed: {}",
            response_str.trim_end_matches('\0')
        );
    }
    Ok(())
}

/// Find an available TCP port for SSH forwarding in range 2222-3000.
pub fn find_available_ssh_port() -> u16 {
    use rand::Rng;
    let mut rng = rand::rng();
    const PORT_RANGE_START: u16 = 2222;
    const PORT_RANGE_END: u16 = 3000;
    for _ in 0..100 {
        let port = rng.random_range(PORT_RANGE_START..PORT_RANGE_END);
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    for port in PORT_RANGE_START..PORT_RANGE_END {
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    PORT_RANGE_START
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_mac() {
        let mac = generate_mac();
        assert_eq!(mac, GVPROXY_STATIC_MAC);
    }

    #[test]
    fn test_find_available_ssh_port() {
        let port = find_available_ssh_port();
        assert!((2222..3000).contains(&port));
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok());
    }

    #[test]
    fn test_ephemeral_vm_metadata_roundtrip() {
        let meta = EphemeralVmMetadata {
            name: "test-vm".to_string(),
            image: "quay.io/fedora/fedora-bootc:42".to_string(),
            pid: 12345,
            gvproxy_pid: 12346,
            ssh_port: 2222,
            ssh_key: "/tmp/test-key".to_string(),
            serial_log: "/tmp/test-serial.log".to_string(),
            log_path: Some("/tmp/test-vfkit.log".to_string()),
            created: "2026-01-01T00:00:00Z".to_string(),
            nbd_unit: Some("bcvk-nbd-test-vm".to_string()),
            nbd_port: Some(10841),
        };
        let json = serde_json::to_string_pretty(&meta).unwrap();
        let loaded: EphemeralVmMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.name, "test-vm");
        assert_eq!(loaded.image, "quay.io/fedora/fedora-bootc:42");
        assert_eq!(loaded.pid, 12345);
        assert_eq!(loaded.nbd_unit.as_deref(), Some("bcvk-nbd-test-vm"));
        assert_eq!(loaded.ssh_port, 2222);
        assert_eq!(loaded.log_path.as_deref(), Some("/tmp/test-vfkit.log"));
    }

    #[test]
    fn test_ephemeral_vm_metadata_save_load_remove() {
        let dir = tempfile::tempdir().unwrap();
        let json_path = dir.path().join("roundtrip-vm.json");
        let meta = EphemeralVmMetadata {
            name: "roundtrip-vm".to_string(),
            image: "localhost/test:latest".to_string(),
            pid: 999,
            gvproxy_pid: 1000,
            ssh_port: 2250,
            ssh_key: "/tmp/key".to_string(),
            serial_log: "/tmp/serial.log".to_string(),
            log_path: None,
            created: "2026-05-04T00:00:00Z".to_string(),
            nbd_unit: None,
            nbd_port: None,
        };
        fs::write(&json_path, serde_json::to_string_pretty(&meta).unwrap()).unwrap();
        let data = fs::read_to_string(&json_path).unwrap();
        let loaded: EphemeralVmMetadata = serde_json::from_str(&data).unwrap();
        assert_eq!(loaded.name, "roundtrip-vm");
        assert_eq!(loaded.ssh_port, 2250);
        assert!(loaded.log_path.is_none());
        fs::remove_file(&json_path).unwrap();
        assert!(!json_path.exists());
    }

    #[test]
    fn test_ephemeral_vm_metadata_list_all_from_dir() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..3 {
            let meta = EphemeralVmMetadata {
                name: format!("vm-{i}"),
                image: "test:latest".to_string(),
                pid: 100 + i,
                gvproxy_pid: 200 + i,
                ssh_port: 2222 + (i as u16),
                ssh_key: "/tmp/key".to_string(),
                serial_log: "/tmp/serial.log".to_string(),
                log_path: None,
                created: "2026-01-01T00:00:00Z".to_string(),
                nbd_unit: Some(format!("bcvk-nbd-vm-{i}")),
                nbd_port: Some(10800 + i as u16),
            };
            let path = dir.path().join(format!("vm-{i}.json"));
            fs::write(&path, serde_json::to_string(&meta).unwrap()).unwrap();
        }
        // Also write a non-json file that should be skipped
        fs::write(dir.path().join("README.txt"), "not json").unwrap();

        let mut vms = Vec::new();
        for entry in fs::read_dir(dir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(meta) = serde_json::from_str::<EphemeralVmMetadata>(&data) {
                    vms.push(meta);
                }
            }
        }
        assert_eq!(vms.len(), 3);
        let mut names: Vec<_> = vms.iter().map(|v| v.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["vm-0", "vm-1", "vm-2"]);
    }
}
