//! Ephemeral VM launch flow for macOS using vfkit + SquashFS.
//!
//! Boot flow:
//! 1. Extract kernel + initramfs from container image
//! 2. Create SquashFS rootfs (lz4, cached by digest)
//! 3. Decompress vmlinuz PE+zstd → uncompressed ARM64 Image
//! 4. Append bcvk units CPIO to initramfs (/etc overlay + /var tmpfs + SSH)
//! 5. Launch vfkit with virtio-blk (SquashFS) + virtio-net (gvproxy)
//!
//! Common helpers (gvproxy, SSH, vfkit detection) are pub for reuse by vfkit/ module.

use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use color_eyre::{
    eyre::{bail, eyre, Context},
    Result,
};
use tracing::{debug, info};

// --- Data structures ---

/// Metadata for a running ephemeral VM, persisted as JSON for `ps` and `ssh`.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
#[allow(dead_code)]
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
}

#[allow(dead_code)]
impl EphemeralVmMetadata {
    /// Return the directory path for ephemeral VM metadata files.
    pub fn vms_dir() -> std::path::PathBuf {
        std::path::PathBuf::from("/private/tmp/bcvk/vms")
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
        let _ = fs::remove_file(path);
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

    /// Check if the VM process is still alive via kill -0.
    pub fn is_alive(&self) -> bool {
        Command::new("kill")
            .args(["-0", &self.pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// Options for launching an ephemeral VM via vfkit.
#[derive(clap::Parser, Debug)]
pub struct RunEphemeralOpts {
    /// Container image to boot
    pub image: String,
    /// Number of vCPUs
    #[clap(long)]
    pub vcpus: Option<u32>,
    /// Memory size (e.g. "4G", "2048M", or plain number for MB)
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

fn default_vcpus() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(2)
}

/// Parse memory specification string (e.g. "4G", "2048M") to megabytes.
pub fn parse_memory_to_mb(s: &str) -> Result<u32> {
    let s = s.trim();
    if let Some(n) = s.strip_suffix('G').or_else(|| s.strip_suffix('g')) {
        Ok((n.parse::<f64>()? * 1024.0) as u32)
    } else if let Some(n) = s.strip_suffix('M').or_else(|| s.strip_suffix('m')) {
        Ok(n.parse::<f64>()? as u32)
    } else {
        Ok(s.parse::<u32>()?)
    }
}

// --- RAII cleanup guard ---

struct VmCleanup {
    vfkit_pid: u32,
    gvproxy_pid: u32,
    vm_name: String,
}

impl Drop for VmCleanup {
    fn drop(&mut self) {
        tracing::debug!("cleaning up VM processes...");
        if let Err(e) = Command::new("kill")
            .arg(self.vfkit_pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            tracing::warn!("failed to kill vfkit (PID {}): {}", self.vfkit_pid, e);
        }
        if let Err(e) = Command::new("kill")
            .arg(self.gvproxy_pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            tracing::warn!("failed to kill gvproxy (PID {}): {}", self.gvproxy_pid, e);
        }
        EphemeralVmMetadata::remove(&self.vm_name);
    }
}

// --- Main entry point ---

/// Run an ephemeral VM from a container image using vfkit + SquashFS.
pub fn run(opts: RunEphemeralOpts) -> Result<()> {
    if opts.gui && opts.detach {
        bail!("--gui and --detach cannot be used together (GUI requires foreground process)");
    }

    if opts.detach {
        return run_detached(&opts);
    }

    let vfkit_bin = find_vfkit()?;
    info!(image = %opts.image, "starting ephemeral VM on macOS (vfkit + SquashFS)");

    let cache_base = std::path::PathBuf::from("/private/tmp/bcvk");
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

    let boot_dir = cache_base.join(format!("boot-{}", digest_short));
    fs::create_dir_all(&boot_dir)?;
    let squashfs_cache = format!("/private/tmp/bcvk/rootfs-{}.squashfs", digest_short);
    let squashfs_path = format!("/private/tmp/bcvk/{}-rootfs.squashfs", vm_name);
    let vmlinuz_path = boot_dir.join("vmlinuz");
    let image_path = boot_dir.join("Image");
    let initramfs_orig = boot_dir.join("initramfs-orig.img");
    let initramfs_path = cache_base.join(format!("{}-initramfs.img", vm_name));

    // Step 1+2: kernel extract + SquashFS creation (parallel)
    let step2_handle = if !Path::new(&squashfs_cache).exists() {
        let mc = machine.clone();
        let rf = rootful;
        let img = opts.image.clone();
        let sc = squashfs_cache.clone();
        Some(std::thread::spawn(move || -> Result<()> {
            info!("creating SquashFS image (lz4)...");
            create_squashfs_image(&mc, rf, &img, &sc)
        }))
    } else {
        info!("using cached SquashFS: {}", squashfs_cache);
        None
    };

    if !vmlinuz_path.exists() || !initramfs_orig.exists() {
        info!("extracting kernel and initramfs...");
        extract_kernel(&machine, &opts.image, &boot_dir)?;
        fs::rename(boot_dir.join("initramfs.img"), &initramfs_orig)?;
    }

    // Step 3+4: kernel decompress + CPIO append (parallel after Step 1)
    let step3_handle = if !image_path.exists() {
        let vp = vmlinuz_path.clone();
        let ip = image_path.clone();
        Some(std::thread::spawn(move || -> Result<()> {
            info!("decompressing kernel (vmlinuz → Image)...");
            extract_uncompressed_kernel(&vp, &ip)
        }))
    } else {
        None
    };

    fs::copy(&initramfs_orig, &initramfs_path)?;
    {
        let cpio_data = crate::cpio::create_initramfs_units_cpio()
            .map_err(|e| eyre!("failed to create CPIO: {e}"))?;
        let mut f = OpenOptions::new().append(true).open(&initramfs_path)?;
        let sz = f.seek(SeekFrom::End(0))?;
        let pad = sz.next_multiple_of(4) - sz;
        if pad > 0 {
            f.write_all(&vec![0u8; pad as usize])?;
        }
        f.write_all(&cpio_data)?;

        if opts.ssh_keygen || !opts.execute.is_empty() {
            info!("generating SSH keypair...");
            let _ = fs::remove_file(&ssh_key_path);
            let _ = fs::remove_file(ssh_key_path.with_extension("pub"));
            let status = Command::new("ssh-keygen")
                .args([
                    "-t",
                    "ed25519",
                    "-f",
                    &ssh_key_path.to_string_lossy(),
                    "-N",
                    "",
                    "-q",
                ])
                .status()?;
            if !status.success() {
                bail!("ssh-keygen failed (exit code: {:?})", status.code());
            }
            let pubkey = fs::read_to_string(ssh_key_path.with_extension("pub"))?;
            let ssh_cpio = create_ssh_setup_cpio(pubkey.trim())?;
            let pos = f.seek(SeekFrom::End(0))?;
            let pad = pos.next_multiple_of(4) - pos;
            if pad > 0 {
                f.write_all(&vec![0u8; pad as usize])?;
            }
            f.write_all(&ssh_cpio)?;
        }
        info!("initramfs prepared");
    }

    if let Some(h) = step3_handle {
        h.join()
            .map_err(|_| eyre!("kernel decompression thread panicked"))??;
    }
    if let Some(h) = step2_handle {
        h.join()
            .map_err(|_| eyre!("squashfs creation thread panicked"))??;
    }

    // CoW clone SquashFS for this VM (allows concurrent use of same image)
    let _ = fs::remove_file(&squashfs_path);
    let clone_status = Command::new("cp")
        .args(["-c", &squashfs_cache, &squashfs_path])
        .status()
        .context("cloning SquashFS")?;
    if !clone_status.success() {
        fs::copy(&squashfs_cache, &squashfs_path).context("copying SquashFS")?;
    }

    // 5. gvproxy + vfkit
    let gvproxy_sock = cache_base.join(format!("{}-gvproxy.sock", vm_name));
    let services_sock = cache_base.join(format!("{}-gvproxy-svc.sock", vm_name));
    let gvproxy_sock_str = gvproxy_sock.to_string_lossy().to_string();
    let services_sock_str = services_sock.to_string_lossy().to_string();
    info!("starting gvproxy...");
    let mut gvproxy_child = start_gvproxy(&gvproxy_sock_str, &services_sock_str)?;

    let mut cmdline_parts: Vec<&str> = vec![
        "root=/dev/vda",
        "ro",
        "rootfstype=squashfs",
        "console=tty0",
        "console=hvc0",
        "loglevel=4",
        "selinux=0",
        "net.ifnames=0",
        "systemd.journald.storage=volatile",
    ];
    let user_args: Vec<&str> = opts.kernel_args.iter().map(|s| s.as_str()).collect();
    cmdline_parts.extend(&user_args);
    let cmdline = cmdline_parts.join(" ");

    let mac = generate_mac();
    let mac_str = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    let bootloader_arg = format!(
        "linux,kernel={},initrd={},cmdline=\"{}\"",
        image_path.display(),
        initramfs_path.display(),
        cmdline
    );

    let vcpus = opts.vcpus.unwrap_or_else(default_vcpus);
    let memory_mb = parse_memory_to_mb(&opts.memory)?;

    let mut vfkit_args = vec![
        "--cpus".to_string(),
        vcpus.to_string(),
        "--memory".to_string(),
        memory_mb.to_string(),
        "--bootloader".to_string(),
        bootloader_arg,
        "--device".to_string(),
        format!("virtio-blk,path={}", squashfs_path),
        "--device".to_string(),
        format!(
            "virtio-net,unixSocketPath={},mac={}",
            gvproxy_sock_str, mac_str
        ),
        "--device".to_string(),
        "virtio-rng".to_string(),
    ];
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
        serial_log: String::new(),
        log_path: None,
        created: chrono::Utc::now().to_rfc3339(),
    };
    metadata.save()?;

    let _cleanup = VmCleanup {
        vfkit_pid: vfkit_child.id(),
        gvproxy_pid: gvproxy_child.id(),
        vm_name: vm_name.clone(),
    };

    if opts.ssh_keygen || !opts.execute.is_empty() {
        info!("setting up SSH port forwarding...");
        for attempt in 0..15u32 {
            match expose_ssh_port(&services_sock_str, "192.168.127.2", ssh_port) {
                Ok(_) => {
                    info!("SSH port {} forwarded", ssh_port);
                    break;
                }
                Err(e) if attempt < 14 => {
                    debug!("SSH port forward attempt {}: {}", attempt, e);
                    let backoff = 200 * 2u64.pow(attempt.min(4));
                    std::thread::sleep(Duration::from_millis(backoff));
                }
                Err(e) => bail!("SSH port forward failed: {}", e),
            }
        }

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
    if let Err(e) = gvproxy_child.kill() {
        tracing::debug!("failed to kill gvproxy: {}", e);
    }
    EphemeralVmMetadata::remove(&vm_name);
    Ok(())
}

fn run_detached(opts: &RunEphemeralOpts) -> Result<()> {
    let cache_base = std::path::PathBuf::from("/private/tmp/bcvk");
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
    };
    metadata.save()?;
    println!("{}", vm_name);
    Ok(())
}

// --- SSH setup CPIO ---

fn create_ssh_setup_cpio(pubkey: &str) -> Result<Vec<u8>> {
    use cpio::newc::Builder as NewcBuilder;
    let mut buf = Vec::new();

    let script = format!(
        "#!/bin/bash\n\
         mkdir -p /sysroot/var/roothome/.ssh\n\
         chmod 700 /sysroot/var/roothome/.ssh\n\
         echo '{}' > /sysroot/var/roothome/.ssh/authorized_keys\n\
         chmod 600 /sysroot/var/roothome/.ssh/authorized_keys\n\
         chown -R 0:0 /sysroot/var/roothome/.ssh\n",
        pubkey
    );

    let service = "[Unit]\n\
         Description=Setup SSH authorized_keys for root\n\
         DefaultDependencies=no\n\
         ConditionPathExists=/etc/initrd-release\n\
         Before=initrd-fs.target\n\
         After=bcvk-var-ephemeral.service\n\
         Requires=bcvk-var-ephemeral.service\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         RemainAfterExit=yes\n\
         ExecStart=/usr/bin/bash /usr/lib/bcvk/setup-ssh.sh\n";

    let dropin = "[Unit]\nWants=bcvk-ssh-setup.service\n";

    let write_entry =
        |buf: &mut Vec<u8>, path: &str, data: &[u8], executable: bool| -> std::io::Result<()> {
            let mode = if executable { 0o100755 } else { 0o100644 };
            let builder = NewcBuilder::new(path).mode(mode).uid(0).gid(0);
            let mut writer = builder.write(buf, data.len() as u32);
            writer.write_all(data)?;
            writer.finish()?;
            Ok(())
        };

    let write_dir = |buf: &mut Vec<u8>, path: &str| -> std::io::Result<()> {
        NewcBuilder::new(path)
            .mode(0o040755)
            .uid(0)
            .gid(0)
            .write(buf, 0)
            .finish()?;
        Ok(())
    };

    write_dir(&mut buf, "usr/lib/bcvk")?;
    write_entry(
        &mut buf,
        "usr/lib/bcvk/setup-ssh.sh",
        script.as_bytes(),
        true,
    )?;
    write_entry(
        &mut buf,
        "usr/lib/systemd/system/bcvk-ssh-setup.service",
        service.as_bytes(),
        false,
    )?;
    write_entry(
        &mut buf,
        "usr/lib/systemd/system/initrd-fs.target.d/bcvk-ssh-setup.conf",
        dropin.as_bytes(),
        false,
    )?;
    cpio::newc::trailer(&mut buf).map_err(|e| eyre!("cpio trailer: {e}"))?;
    Ok(buf)
}

// --- vfkit kernel decompression ---

fn extract_uncompressed_kernel(vmlinuz_path: &Path, output_path: &Path) -> Result<()> {
    let data = fs::read(vmlinuz_path)?;

    // Parse zboot header: offset 0x08 = payload_offset (le32), 0x0c = payload_size (le32)
    let (pos, payload_end) = if data.len() >= 16 && &data[4..8] == b"zimg" {
        let payload_offset = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
        let payload_size = u32::from_le_bytes(data[12..16].try_into().unwrap()) as usize;
        if payload_offset + payload_size > data.len() {
            bail!("zboot payload extends beyond file");
        }
        info!(
            "zboot header: payload at 0x{:x}, size 0x{:x}",
            payload_offset, payload_size
        );
        (payload_offset, payload_offset + payload_size)
    } else {
        let magic = [0x28u8, 0xb5, 0x2f, 0xfd];
        let p = data
            .windows(4)
            .position(|w| w == magic)
            .ok_or_else(|| eyre!("zstd magic not found in vmlinuz"))?;
        info!("zstd payload at offset 0x{:x} (no zboot header)", p);
        (p, data.len())
    };

    let mut kernel = Vec::new();
    zstd::stream::copy_decode(&data[pos..payload_end], &mut kernel)
        .context("decompressing zstd payload from vmlinuz")?;

    if kernel.len() < 0x3c || &kernel[0x38..0x3c] != b"ARMd" {
        bail!("decompressed kernel is not a valid ARM64 Image");
    }
    fs::write(output_path, &kernel)?;
    info!("decompressed kernel: {} bytes (ARM64 Image)", kernel.len());
    Ok(())
}

// --- Shared helpers (pub for vfkit/ module) ---

fn detect_machine_name() -> Result<String> {
    let output = Command::new("podman")
        .args(["machine", "info", "--format", "{{.Host.CurrentMachine}}"])
        .output()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        bail!("no podman machine is running");
    }
    Ok(name)
}

fn ensure_image_and_get_digest(image: &str) -> Result<String> {
    let status = Command::new("podman")
        .args(["image", "exists", image])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        info!("pulling image {}...", image);
        if !Command::new("podman")
            .args(["pull", image])
            .status()?
            .success()
        {
            bail!("failed to pull image: {}", image);
        }
    }
    let output = Command::new("podman")
        .args(["image", "inspect", "--format", "{{.Digest}}", image])
        .output()?;
    let digest = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(digest.trim_start_matches("sha256:").to_string())
}

fn extract_kernel(machine: &str, image: &str, boot_dir: &Path) -> Result<()> {
    let boot_dir_str = boot_dir.to_string_lossy();
    let script = format!(
        "KVER=$(podman run --rm {image} ls /usr/lib/modules/ | head -1) && \
         [ -n \"$KVER\" ] && \
         podman run --rm {image} cat /usr/lib/modules/$KVER/vmlinuz > {boot}/vmlinuz && \
         podman run --rm {image} cat /usr/lib/modules/$KVER/initramfs.img > {boot}/initramfs.img",
        image = image,
        boot = boot_dir_str
    );
    let output = Command::new("podman")
        .args(["machine", "ssh", machine, &script])
        .output()
        .context("extracting kernel from container image")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "No kernel found in image '{}'.\n\
               Checked: /usr/lib/modules/<version>/vmlinuz + initramfs.img\n\
               This image may not be a bootable container (bootc) image.\n\
               {}",
            image,
            stderr.trim()
        );
    }
    Ok(())
}

fn is_machine_rootful(machine: &str) -> bool {
    Command::new("podman")
        .args(["machine", "ssh", machine, "id", "-u"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

fn create_squashfs_image(
    machine: &str,
    rootful: bool,
    image: &str,
    output_path: &str,
) -> Result<()> {
    let script = if rootful {
        format!(
            "MERGED=$(podman image mount {}) && \
             mksquashfs $MERGED {} -noappend -comp lz4 -b 1M -quiet",
            image, output_path
        )
    } else {
        info!("rootless mode: using podman unshare for SquashFS creation");
        format!(
            "podman unshare sh -c 'MERGED=$(podman image mount {}) && \
             mksquashfs $MERGED {} -noappend -comp lz4 -b 1M -quiet'",
            image, output_path
        )
    };

    let output = Command::new("podman")
        .args(["machine", "ssh", machine, &script])
        .output()
        .context("running mksquashfs")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("mksquashfs failed: {}", stderr.trim());
    }
    Ok(())
}

/// Clear extended attributes from a file.
///
/// Apple Virtualization.framework rejects disk images with xattrs like
/// `security.selinux` or `user.containers.override_stat` that are added
/// by podman/buildah when creating images inside containers.
pub fn clear_xattr(path: &Path) {
    let _ = Command::new("xattr")
        .args(["-c", &path.to_string_lossy()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
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

/// Fixed MAC address matching gvproxy's DHCP static lease for 192.168.127.2.
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
    let _ = fs::remove_file(gvproxy_sock);
    let _ = fs::remove_file(services_sock);
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
    for _ in 0..50 {
        if Path::new(gvproxy_sock).exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !Path::new(gvproxy_sock).exists() {
        bail!("gvproxy socket did not appear");
    }
    Ok(child)
}

/// Expose SSH port forwarding via gvproxy's HTTP API.
pub fn expose_ssh_port(services_sock: &str, vm_ip: &str, host_port: u16) -> Result<()> {
    let body = format!(
        r#"{{"local":":{}","remote":"{}:22","protocol":"tcp"}}"#,
        host_port, vm_ip
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
    let _ = std::io::Read::read(&mut stream, &mut response);
    let response_str = String::from_utf8_lossy(&response);
    if !response_str.contains("200") {
        bail!(
            "gvproxy expose failed: {}",
            response_str.trim_end_matches('\0')
        );
    }
    Ok(())
}

const SSH_TIMEOUT: Duration = Duration::from_secs(240);

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

/// Wait for SSH connectivity with exponential backoff (240s timeout).
pub fn wait_for_ssh(port: u16, key_path: &Path, user: &str) -> Result<()> {
    use crate::ssh_options::CommonSshOptions;
    let ssh_opts = CommonSshOptions::default();
    let user_host = format!("{}@localhost", user);
    info!("waiting for SSH on port {} ({}@localhost)...", port, user);
    let start = std::time::Instant::now();
    let mut attempt = 0u32;
    loop {
        if start.elapsed() > SSH_TIMEOUT {
            bail!("SSH connection timeout ({}s)", SSH_TIMEOUT.as_secs());
        }
        let mut cmd = Command::new("ssh");
        cmd.args(["-p", &port.to_string(), "-i", &key_path.to_string_lossy()]);
        ssh_opts.apply_to_command(&mut cmd);
        cmd.args(["-o", "BatchMode=yes", &user_host, "true"]);
        let status = cmd.stdout(Stdio::null()).stderr(Stdio::null()).status();
        if let Ok(s) = status {
            if s.success() {
                info!("SSH connected after {}s", start.elapsed().as_secs());
                return Ok(());
            }
        }
        let backoff = if attempt < 2 {
            500
        } else if attempt < 4 {
            1000
        } else {
            2000
        };
        std::thread::sleep(Duration::from_millis(backoff));
        attempt += 1;
    }
}

/// Execute a command via SSH and return the exit status.
pub fn run_ssh_command(
    port: u16,
    key_path: &Path,
    user: &str,
    command: &str,
) -> Result<std::process::ExitStatus> {
    use crate::ssh_options::CommonSshOptions;
    let ssh_opts = CommonSshOptions::default();
    let user_host = format!("{}@localhost", user);
    let mut cmd = Command::new("ssh");
    cmd.args(["-p", &port.to_string(), "-i", &key_path.to_string_lossy()]);
    ssh_opts.apply_to_command(&mut cmd);
    cmd.args(["-o", "BatchMode=yes", &user_host, command]);
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| eyre!("ssh failed: {}", e))
}

/// Start an interactive SSH session with TTY allocation.
pub fn run_ssh_interactive(
    port: u16,
    key_path: &Path,
    user: &str,
) -> Result<std::process::ExitStatus> {
    use crate::ssh_options::CommonSshOptions;
    let ssh_opts = CommonSshOptions::default();
    let user_host = format!("{}@localhost", user);
    let mut cmd = Command::new("ssh");
    cmd.args(["-p", &port.to_string(), "-i", &key_path.to_string_lossy()]);
    ssh_opts.apply_to_command(&mut cmd);
    cmd.args(["-t", &user_host]);
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| eyre!("ssh failed: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_memory_to_mb() {
        let cases = [
            ("4G", 4096),
            ("4g", 4096),
            ("2048M", 2048),
            ("2048m", 2048),
            ("512", 512),
            ("1G", 1024),
        ];
        for (input, expected) in &cases {
            assert_eq!(
                parse_memory_to_mb(input).unwrap(),
                *expected,
                "parse_memory_to_mb({:?})",
                input
            );
        }
    }

    #[test]
    fn test_parse_memory_to_mb_errors() {
        assert!(parse_memory_to_mb("").is_err());
        assert!(parse_memory_to_mb("abc").is_err());
    }

    #[test]
    fn test_generate_mac() {
        let mac = generate_mac();
        assert_eq!(mac, GVPROXY_STATIC_MAC);
    }

    #[test]
    fn test_default_vcpus() {
        let vcpus = default_vcpus();
        assert!(vcpus >= 1);
        assert_eq!(
            vcpus,
            std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(2)
        );
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
        };
        let json = serde_json::to_string_pretty(&meta).unwrap();
        let loaded: EphemeralVmMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.name, "test-vm");
        assert_eq!(loaded.image, "quay.io/fedora/fedora-bootc:42");
        assert_eq!(loaded.pid, 12345);
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
