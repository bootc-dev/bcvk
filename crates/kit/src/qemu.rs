//! QEMU virtualization integration and VM management.
//!
//! Supports direct kernel boot with VirtIO devices, automatic process cleanup,
//! and SMBIOS credential injection.

use std::fs::{File, OpenOptions};
use std::future::Future;
use std::io::ErrorKind;
use std::os::fd::{AsRawFd as _, OwnedFd};
use std::os::unix::process::CommandExt as _;
use std::pin::Pin;
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std_ext::cmdext::CapStdExtCommandExt;
use color_eyre::eyre::{eyre, Context};
use color_eyre::Result;
use libc::{VMADDR_CID_ANY, VMADDR_PORT_ANY};
use nix::sys::socket::{accept, bind, getsockname, socket, AddressFamily, SockFlag, SockType};
use tracing::{debug, trace, warn};
use vsock::VsockAddr;

/// The device for vsock allocation
pub const VHOST_VSOCK: &str = "/dev/vhost-vsock";

/// VirtIO-FS mount point configuration.
#[derive(Debug, Clone)]
pub struct VirtiofsMount {
    /// Unix socket path for virtiofsd communication
    pub socket_path: String,
    /// Mount tag used by guest to identify this mount
    pub tag: String,
}

/// VirtIO-Serial device for guest-to-host communication.
/// Appears as /dev/virtio-ports/{name} in guest.
#[derive(Debug)]
pub struct VirtioSerialOut {
    /// Device name (becomes /dev/virtio-ports/{name})
    pub name: String,
    /// Host file path for output
    pub output_file: String,
    /// Whether to append to the file (needed for fdsets)
    pub append: bool,
}

/// VirtIO-Block storage device configuration.
/// Appears as /dev/disk/by-id/virtio-{serial} in guest.
#[derive(Debug)]
pub struct VirtioBlkDevice {
    /// Host disk image file path
    pub disk_file: String,
    /// Device serial for guest identification
    pub serial: String,
    /// Disk image format
    pub format: crate::to_disk::Format,
}

/// VM display and console configuration.
#[derive(Debug, Clone)]
pub enum DisplayMode {
    /// Headless mode (-nographic)
    None,
    /// Console to stdio (-serial stdio -display none)
    Console,
}

impl Default for DisplayMode {
    fn default() -> Self {
        DisplayMode::None
    }
}

/// VM network configuration.
#[derive(Debug, Clone)]
pub enum NetworkMode {
    /// User-mode networking with NAT and port forwarding
    User {
        /// Port forwarding rules: "tcp::2222-:22" format
        hostfwd: Vec<String>,
    },
}

impl Default for NetworkMode {
    fn default() -> Self {
        NetworkMode::User { hostfwd: vec![] }
    }
}

/// Resource limits for QEMU processes.
/// Note: Applied externally via taskset/ionice/nice, not QEMU args.
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// CPU affinity bitmask ("0xF" for cores 0-3)
    pub cpu_affinity: Option<String>,
    /// I/O priority (0=highest, 7=lowest)
    pub io_priority: Option<u8>,
    /// Nice level (-20=highest, 19=lowest)
    pub nice_level: Option<i8>,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            cpu_affinity: None,
            io_priority: None,
            nice_level: None,
        }
    }
}

/// VM boot configuration: direct kernel boot.
#[derive(Debug)]
pub enum BootMode {
    /// Direct kernel boot (fast, testing-focused)
    /// Also used for UKI boot after extracting kernel/initramfs from UKI PE sections.
    ///
    /// Note: For UKI images, we extract kernel/initramfs using objcopy rather than
    /// booting the UKI directly via OVMF. This allows us to append bcvk units to
    /// the initramfs for /etc overlay and /var setup. The tradeoff is that this
    /// breaks the UKI signature chain, so Secure Boot is not supported for
    /// ephemeral runs. See https://github.com/bootc-dev/bcvk/issues/161 for
    /// future work on UEFI boot support.
    DirectBoot {
        kernel_path: String,
        initramfs_path: String,
        kernel_cmdline: Vec<String>,
        /// VirtIO-FS socket for root filesystem
        virtiofs_socket: Utf8PathBuf,
    },
}

/// Complete QEMU VM configuration with builder pattern.
#[derive(Default, Debug)]
pub struct QemuConfig {
    /// RAM in megabytes (128MB-1TB)
    pub memory_mb: u32,
    /// Number of vCPUs (1-256)
    pub vcpus: u32,
    boot_mode: Option<BootMode>,
    /// Main VirtioFS configuration for root filesystem (handled separately from additional mounts)
    pub main_virtiofs_config: Option<VirtiofsConfig>,
    /// VirtioFS configurations to spawn as daemons
    pub virtiofs_configs: Vec<VirtiofsConfig>,
    /// File descriptors to pass
    fdset: Vec<Arc<OwnedFd>>,
    /// Additional VirtIO-FS mounts
    pub additional_mounts: Vec<VirtiofsMount>,
    pub virtio_serial_devices: Vec<VirtioSerialOut>,
    pub virtio_blk_devices: Vec<VirtioBlkDevice>,
    pub display_mode: DisplayMode,
    pub network_mode: NetworkMode,
    pub resource_limits: ResourceLimits,
    /// Deprecated: use display_mode
    pub enable_console: bool,
    /// SMBIOS credentials for systemd
    smbios_credentials: Vec<String>,

    /// Write systemd notifications to this file
    pub systemd_notify: Option<File>,

    vhost_fd: Option<File>,
}

impl QemuConfig {
    /// Create a new config with direct boot (kernel + initramfs)
    pub fn new_direct_boot(
        memory_mb: u32,
        vcpus: u32,
        kernel_path: String,
        initramfs_path: String,
        virtiofs_socket: Utf8PathBuf,
    ) -> Self {
        Self {
            memory_mb,
            vcpus,
            boot_mode: Some(BootMode::DirectBoot {
                kernel_path,
                initramfs_path,
                kernel_cmdline: vec![],
                virtiofs_socket,
            }),
            ..Default::default()
        }
    }

    // Enable vsock
    pub fn enable_vsock(&mut self) -> Result<()> {
        let fd = OpenOptions::new()
            .read(true)
            .write(true)
            .open(VHOST_VSOCK)
            .with_context(|| format!("Failed to open {VHOST_VSOCK} for CID allocation"))?;
        self.vhost_fd = Some(fd);
        Ok(())
    }

    /// Set kernel command line arguments (only for direct boot)
    pub fn set_kernel_cmdline(&mut self, cmdline: Vec<String>) -> &mut Self {
        if let Some(BootMode::DirectBoot { kernel_cmdline, .. }) = self.boot_mode.as_mut() {
            *kernel_cmdline = cmdline;
        }
        self
    }

    /// Enable console output
    pub fn set_console(&mut self, enable: bool) -> &mut Self {
        self.enable_console = enable;
        if enable {
            self.display_mode = DisplayMode::Console;
        }
        self
    }

    /// Validate configuration before VM creation
    pub fn validate(&self) -> Result<()> {
        // Memory validation
        if self.memory_mb < 128 {
            return Err(eyre!(
                "Memory too low: {}MB (minimum 128MB)",
                self.memory_mb
            ));
        }
        if self.memory_mb > 1024 * 1024 {
            return Err(eyre!("Memory too high: {}MB (maximum 1TB)", self.memory_mb));
        }

        // CPU validation
        if self.vcpus == 0 {
            return Err(eyre!("vCPU count must be at least 1"));
        }
        if self.vcpus > 256 {
            return Err(eyre!("vCPU count too high: {} (maximum 256)", self.vcpus));
        }

        // Validate virtiofs mounts
        for mount in &self.additional_mounts {
            if mount.tag.is_empty() {
                return Err(eyre!("Virtiofs mount tag cannot be empty"));
            }
            let socket_dir = std::path::Path::new(&mount.socket_path)
                .parent()
                .ok_or_else(|| eyre!("Invalid virtiofs socket path: {}", mount.socket_path))?;
            if !socket_dir.exists() {
                return Err(eyre!(
                    "Virtiofs socket directory does not exist: {}",
                    socket_dir.display()
                ));
            }
        }

        Ok(())
    }

    /// Add a virtio-blk device with specified format
    pub fn add_virtio_blk_device_with_format(
        &mut self,
        disk_file: String,
        serial: String,
        format: crate::to_disk::Format,
    ) -> &mut Self {
        self.virtio_blk_devices.push(VirtioBlkDevice {
            disk_file,
            serial,
            format,
        });
        self
    }

    /// Set the main virtiofs configuration for the root filesystem
    pub fn set_main_virtiofs(&mut self, config: VirtiofsConfig) -> &mut Self {
        self.main_virtiofs_config = Some(config);
        self
    }

    /// Add a virtiofs configuration that will be spawned as a daemon
    pub fn add_virtiofs(&mut self, config: VirtiofsConfig, tag: &str) -> &mut Self {
        // Also add a corresponding mount so QEMU knows about it
        self.additional_mounts.push(VirtiofsMount {
            socket_path: config.socket_path.clone().into(),
            tag: tag.to_owned(),
        });
        self.virtiofs_configs.push(config);
        self
    }

    /// Add a virtio-serial output device
    pub fn add_virtio_serial_out(
        &mut self,
        name: &str,
        output_file: String,
        append: bool,
    ) -> &mut Self {
        self.virtio_serial_devices.push(VirtioSerialOut {
            name: name.to_owned(),
            output_file,
            append,
        });
        self
    }

    pub fn add_fd(&mut self, fd: Arc<OwnedFd>) -> String {
        self.fdset.push(fd);
        format!("/dev/fdset/{}", self.fdset.len())
    }

    /// Create a virtio-serial device with pipe-based output
    pub fn add_virtio_serial_pipe(&mut self, name: &str) -> Result<OwnedFd> {
        // Create a pipe for QEMU chardev communication
        use rustix::pipe::pipe;
        let (read_fd, write_fd) = pipe().context("Failed to create pipe")?;

        let fdset = self.add_fd(Arc::new(write_fd));

        // Use append=true for fdsets to avoid truncation issues
        self.add_virtio_serial_out(name, fdset, true);
        Ok(read_fd)
    }

    /// Add SMBIOS credential for systemd credential passing
    pub fn add_smbios_credential(&mut self, credential: String) -> &mut Self {
        self.smbios_credentials.push(credential);
        self
    }

    /// Enable SSH access by configuring port forwarding
    pub fn enable_ssh_access(&mut self, host_port: Option<u16>) -> &mut Self {
        let port = host_port.unwrap_or(2222); // Default to port 2222 on host
        let hostfwd = format!("tcp::{}-:22", port); // Forward host port to guest port 22
        self.network_mode = NetworkMode::User {
            hostfwd: vec![hostfwd],
        };
        self
    }
}

/// Allocate a unique VSOCK CID
fn allocate_vsock_cid(vhost_fd: File) -> Result<(OwnedFd, u32)> {
    use std::os::unix::io::AsRawFd;

    for candidate_cid in 3..10001u32 {
        // Test if this CID is available
        // VHOST_VSOCK_SET_GUEST_CID = _IOW(VHOST_VIRTIO, 0x60, __u64)
        const VHOST_VSOCK_SET_GUEST_CID: libc::c_ulong = 0x4008af60;

        let cid = candidate_cid as u64;
        // SAFETY: ioctl is unsafe but we're passing valid file descriptor and pointer
        #[allow(unsafe_code)]
        let result = unsafe {
            match libc::ioctl(
                vhost_fd.as_raw_fd(),
                VHOST_VSOCK_SET_GUEST_CID,
                &cid as *const u64,
            ) {
                0 => Ok(()),
                _ => Err(std::io::Error::last_os_error()),
            }
        };
        match result {
            Ok(()) => {
                // Success! This CID is available
                debug!("Successfully allocated VSOCK CID: {}", candidate_cid);
                return Ok((vhost_fd.into(), candidate_cid));
            }
            Err(e) if e.kind() == ErrorKind::AddrInUse => {
                debug!("VSOCK CID {} is in use, trying next", candidate_cid);
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }

    Err(eyre!("Could not find available VSOCK CID (tried 3-10000)"))
}

/// Spawn QEMU VM process with given configuration and optional extra credential.
/// Uses KVM acceleration, memory-backend-memfd for VirtIO-FS compatibility.
fn spawn(
    config: &QemuConfig,
    extra_credentials: &[String],
    vsock: Option<(OwnedFd, u32)>,
) -> Result<Child> {
    // Validate configuration first
    config.validate()?;
    let memory_arg = format!("{}M", config.memory_mb);
    let memory_obj_arg = format!(
        "memory-backend-memfd,id=mem,share=on,size={}M",
        config.memory_mb
    );

    let qemu = std::env::var("QEMU_BIN")
        .ok()
        .map(Ok)
        .unwrap_or_else(|| -> Result<_> {
            // RHEL only supports non-emulated, and qemu is an implementation detail
            // of higher level virt.
            let libexec_qemu = Utf8Path::new("/usr/libexec/qemu-kvm");
            if libexec_qemu.try_exists()? {
                Ok(libexec_qemu.to_string())
            } else {
                let arch = std::env::consts::ARCH;
                Ok(format!("qemu-system-{arch}"))
            }
        })
        .context("Checking for qemu")?;

    let mut cmd = Command::new(qemu);
    // SAFETY: This API is safe to call in a forked child.
    #[allow(unsafe_code)]
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::TERM))
                .map_err(Into::into)
        });
    }
    cmd.args([
        "-m",
        &memory_arg,
        "-smp",
        &config.vcpus.to_string(),
        "-enable-kvm",
        "-cpu",
        "host",
        "-audio",
        "none",
        "-object",
        &memory_obj_arg,
        "-numa",
        "node,memdev=mem",
    ]);

    for (idx, fd) in config.fdset.iter().enumerate() {
        let fd_id = 100 + idx as u32; // Start at 100 to avoid conflicts
        let set_id = idx + 1; // fdset starts at 1

        // Pass the write FD to QEMU
        cmd.take_fd_n(Arc::clone(fd), fd_id as i32);

        cmd.args(["-add-fd", &format!("fd={},set={}", fd_id, set_id)]);
    }

    // Add virtio-blk block devices
    for (idx, blk_device) in config.virtio_blk_devices.iter().enumerate() {
        let drive_id = format!("drive{}", idx);
        cmd.args([
            "-drive",
            &format!(
                "file={},format={},if=none,id={}",
                blk_device.disk_file,
                blk_device.format.as_str(),
                drive_id
            ),
            "-device",
            &format!(
                "virtio-blk-pci,drive={},serial={}",
                drive_id, blk_device.serial
            ),
        ]);
    }

    // Configure boot mode
    match config.boot_mode.as_ref() {
        Some(BootMode::DirectBoot {
            kernel_path,
            initramfs_path,
            kernel_cmdline,
            virtiofs_socket,
        }) => {
            // Direct kernel boot
            cmd.args(["-kernel", kernel_path, "-initrd", initramfs_path]);

            // Add virtiofs root mount for direct boot
            cmd.args([
                "-chardev",
                &format!("socket,id=char0,path={}", virtiofs_socket),
                "-device",
                "vhost-user-fs-pci,queue-size=1024,chardev=char0,tag=rootfs",
            ]);

            // Add kernel command line
            let append_str = kernel_cmdline.join(" ");
            cmd.args(["-append", &append_str]);
        }
        None => {}
    }

    // Add additional virtiofs mounts
    for (idx, mount) in config.additional_mounts.iter().enumerate() {
        let char_id = format!("char{}", idx + 1);
        cmd.args([
            "-chardev",
            &format!("socket,id={},path={}", char_id, mount.socket_path),
            "-device",
            &format!(
                "vhost-user-fs-pci,queue-size=1024,chardev={},tag={}",
                char_id, mount.tag
            ),
        ]);
    }

    // Add virtio-serial controller - always needed for console
    cmd.args(["-device", "virtio-serial"]);

    // Add virtio-serial devices
    for (idx, serial_device) in config.virtio_serial_devices.iter().enumerate() {
        let char_id = format!("serial_char{}", idx);
        // Build chardev args with optional append
        let chardev_args = if serial_device.append {
            format!(
                "file,id={},path={},append=on",
                char_id, serial_device.output_file
            )
        } else {
            format!("file,id={},path={}", char_id, serial_device.output_file)
        };

        cmd.args([
            "-chardev",
            &chardev_args,
            "-device",
            &format!(
                "virtserialport,chardev={},name={}",
                char_id, serial_device.name
            ),
        ]);
    }

    // Configure network (only User mode supported now)
    match &config.network_mode {
        NetworkMode::User { hostfwd } => {
            let mut netdev_parts = vec!["user".to_string(), "id=net0".to_string()];

            // Add port forwarding rules
            for fwd in hostfwd {
                netdev_parts.push(format!("hostfwd={}", fwd));
            }

            let netdev_arg = netdev_parts.join(",");
            cmd.args([
                "-netdev",
                &netdev_arg,
                "-device",
                "virtio-net-pci,netdev=net0",
            ]);
        }
    }

    // No GUI, and no emulated serial ports by default.
    cmd.args(["-serial", "none", "-nographic", "-display", "none"]);

    match &config.display_mode {
        DisplayMode::None => {
            // Disable monitor in non-console mode
            cmd.args(["-monitor", "none"]);
        }
        DisplayMode::Console => {
            cmd.args(["-device", "virtconsole,chardev=console0"]);
            cmd.args(["-chardev", "stdio,id=console0,mux=on"]);
            // Use monitor on the same muxed chardev
            cmd.args(["-monitor", "chardev:console0"]);
        }
    }

    // Apply resource limits
    if let Some(affinity) = &config.resource_limits.cpu_affinity {
        // Note: CPU affinity is typically set via taskset or systemd, not QEMU args
        debug!("CPU affinity requested: {} (apply externally)", affinity);
    }

    if let Some(io_priority) = config.resource_limits.io_priority {
        // Note: I/O priority is typically set via ionice, not QEMU args
        debug!("I/O priority requested: {} (apply externally)", io_priority);
    }

    if let Some(nice_level) = config.resource_limits.nice_level {
        // Note: Nice level is typically set via nice command, not QEMU args
        debug!("Nice level requested: {} (apply externally)", nice_level);
    }

    // Add AF_VSOCK device if enabled
    if let Some((vhostfd, guest_cid)) = vsock {
        debug!("Adding AF_VSOCK device with guest CID: {}", guest_cid);
        cmd.take_fd_n(Arc::new(vhostfd), 42);
        cmd.args([
            "-device",
            &format!("vhost-vsock-pci,guest-cid={},vhostfd=42", guest_cid),
        ]);
    }

    // Add SMBIOS credentials for systemd credential passing
    for credential in &config.smbios_credentials {
        cmd.args(["-smbios", &format!("type=11,value={}", credential)]);
    }

    // Add extra credentials passed to this function
    for credential in extra_credentials {
        cmd.args(["-smbios", &format!("type=11,value={}", credential)]);
    }

    // Configure stdio based on display mode
    match &config.display_mode {
        DisplayMode::Console => {
            // Keep stdio for console interaction
        }
        _ => {
            // Redirect stdout/stderr for non-console modes
            if !config.enable_console {
                // In non-console mode, redirect stderr to inherited (so we can see QEMU errors)
                // but redirect stdout to null (to avoid noise)
                cmd.stdout(Stdio::null()).stderr(Stdio::inherit());
            }
        }
    }

    tracing::debug!("{cmd:?}");

    cmd.spawn().context("Failed to spawn QEMU")
}

struct VsockCopier {
    port: VsockAddr,
    #[allow(dead_code)]
    copier: std::thread::JoinHandle<Result<()>>,
}

pub struct RunningQemu {
    pub qemu_process: Child,
    #[allow(dead_code)]
    pub virtiofsd_processes: Vec<Pin<Box<dyn Future<Output = std::io::Result<Output>>>>>,
    #[allow(dead_code)]
    sd_notification: Option<VsockCopier>,
}

impl RunningQemu {
    /// Spawn QEMU
    pub async fn spawn(mut config: QemuConfig) -> Result<Self> {
        // Spawn all virtiofsd processes first
        let mut awaiting_virtiofsd = Vec::new();
        let virtiofsd_configs = config
            .main_virtiofs_config
            .iter()
            .chain(config.virtiofs_configs.iter());
        for config in virtiofsd_configs {
            let process = spawn_virtiofsd_async(config).await?;
            awaiting_virtiofsd.push((process, config.socket_path.clone()));
        }

        // Wait for all virtiofsd to be ready
        let mut virtiofsd_processes = Vec::new();
        while let Some((proc, socket_path)) = awaiting_virtiofsd.pop() {
            let socket_path = &socket_path;
            let query_exists = async move {
                loop {
                    if socket_path.exists() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            };
            tokio::pin!(query_exists);
            let timeout_val = Duration::from_secs(60);
            let timeout = tokio::time::sleep(timeout_val);
            tokio::pin!(timeout);
            debug!("Waiting for socket at {socket_path}");
            let mut output: Pin<Box<dyn Future<Output = std::io::Result<Output>>>> =
                Box::pin(proc.wait_with_output());
            tokio::select! {
                output = &mut output => {
                    tracing::trace!("virtiofsd exited");
                    let output = output?;
                    let status = output.status;
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(eyre!(
                        "virtiofsd failed to start for socket {socket_path}\nExit status: {status:?}\nOutput: {stderr}"
                    ));
                }
                _ = timeout => {
                    return Err(eyre!("timed out waiting for virtiofsd socket {} to be created (waited {timeout_val:?})", socket_path));
                }
                _ = query_exists => {
                }
            }
            virtiofsd_processes.push(output);
            tracing::debug!("virtiofsd socket created: {socket_path}");
        }

        let vsockdata = if let Some(vhost_fd) = config.vhost_fd.take() {
            // Get a unique guest CID using dynamic allocation
            // If /dev/vhost-vsock is not available, fall back to disabled vsock
            match allocate_vsock_cid(vhost_fd) {
                Ok(data) => Some(data),
                Err(e) => {
                    debug!("Failed to allocate vsock CID, disabling vsock: {}", e);
                    None
                }
            }
        } else {
            None
        };

        let sd_notification = if let Some(target) = config.systemd_notify.take() {
            color_eyre::eyre::ensure!(vsockdata.is_some());
            let vsock = socket(
                AddressFamily::Vsock,
                SockType::Stream,
                SockFlag::SOCK_CLOEXEC,
                None,
            )
            .map_err(|e| eyre!("Failed to create AF_VSOCK stream socket: {}", e))?;

            // Bind to host address with ANY port - let kernel allocate a free port
            let addr = VsockAddr::new(VMADDR_CID_ANY, VMADDR_PORT_ANY);
            bind(vsock.as_raw_fd(), &addr)
                .map_err(|e| eyre!("Failed to bind AF_VSOCK stream socket: {}", e))?;

            let port = getsockname(vsock.as_raw_fd())?;
            debug!("Listening on AF_VSOCK {port}");

            // Start listening before spawning the thread
            nix::sys::socket::listen(&vsock, nix::sys::socket::Backlog::new(5).unwrap())
                .map_err(|e| eyre!("Failed to listen on AF_VSOCK: {}", e))?;

            let copier = std::thread::spawn(move || -> Result<()> {
                use std::io::Write;

                debug!("AF_VSOCK listener thread started, waiting for systemd notifications");
                let mut target = target;

                // Accept connections and copy data to target file
                loop {
                    match accept(vsock.as_raw_fd()) {
                        Ok(client_fd) => {
                            debug!("Accepted systemd notification connection");

                            // Read from socket and write to file
                            let mut buffer = [0u8; 4096];
                            match nix::sys::socket::recv(
                                client_fd,
                                &mut buffer,
                                nix::sys::socket::MsgFlags::empty(),
                            ) {
                                Ok(bytes_read) if bytes_read > 0 => {
                                    let data = &buffer[..bytes_read];
                                    trace!("Received systemd notification ({} bytes)", bytes_read);

                                    // Write raw data directly to target file
                                    target.write_all(data)?;
                                    target.write_all(b"\n")?; // Add newline to separate notifications
                                    target.flush()?;
                                }
                                Ok(_) => {
                                    debug!("Connection closed");
                                }
                                Err(e) => {
                                    warn!("Failed to receive data: {}", e);
                                }
                            }

                            // Close client connection
                            let _ = nix::unistd::close(client_fd);
                        }
                        Err(nix::errno::Errno::EAGAIN) => {
                            // No connection available, sleep briefly
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        Err(e) => {
                            warn!("Failed to accept connection: {}", e);
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }
                }
            });

            Some(VsockCopier { port, copier })
        } else {
            None
        };

        let creds = sd_notification
            .as_ref()
            .map(|sd| {
                let cred = crate::credentials::smbios_cred_for_vsock_notify(2, sd.port.port());
                vec![cred]
            })
            .unwrap_or_default();

        // Spawn QEMU process with additional VSOCK credential if needed
        let qemu_process = spawn(&config, &creds, vsockdata)?;

        Ok(Self {
            qemu_process,
            virtiofsd_processes,
            sd_notification,
        })
    }

    /// Wait for QEMU process to exit
    pub async fn wait(&mut self) -> Result<std::process::ExitStatus> {
        let r = self.qemu_process.wait()?;
        Ok(r)
    }
}

/// Spawn QEMU with automatic process cleanup via guard.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_virtio_serial_device_creation() {
        let mut config = QemuConfig::new_direct_boot(
            1024,
            1,
            "/test/kernel".to_string(),
            "/test/initramfs".to_string(),
            "/test/socket".into(),
        );
        config
            .add_virtio_serial_out("serial0", "/tmp/output.txt".to_string(), false)
            .set_kernel_cmdline(vec!["console=ttyS0".to_string()])
            .set_console(true);

        // Test that the config is created correctly
        assert_eq!(config.virtio_serial_devices.len(), 1);
        assert_eq!(config.virtio_serial_devices[0].name, "serial0");
        assert_eq!(
            config.virtio_serial_devices[0].output_file,
            "/tmp/output.txt"
        );
    }
}

/// VirtiofsD daemon configuration.
#[derive(Debug, Clone)]
pub struct VirtiofsConfig {
    /// Unix socket for QEMU communication
    pub socket_path: Utf8PathBuf,
    /// Host directory to share
    pub shared_dir: Utf8PathBuf,
    pub debug: bool,
    /// Mount as read-only
    pub readonly: bool,
    /// Optional log file path for virtiofsd output
    pub log_file: Option<Utf8PathBuf>,
}

impl Default for VirtiofsConfig {
    fn default() -> Self {
        Self {
            socket_path: "/run/inner-shared/virtiofs.sock".into(),
            shared_dir: "/run/source-image".into(),
            debug: false,
            // We don't need to write to this, there's a transient overlay
            readonly: true,
            log_file: None,
        }
    }
}

/// Check if virtiofsd supports the --readonly flag.
async fn virtiofsd_supports_readonly(virtiofsd_binary: &str) -> bool {
    let output = tokio::process::Command::new(virtiofsd_binary)
        .arg("--help")
        .output()
        .await;

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            stdout.contains("--readonly") || stderr.contains("--readonly")
        }
        Err(_) => false,
    }
}

/// Spawn virtiofsd daemon process as tokio::process::Child.
/// Searches for binary in /usr/libexec, /usr/bin, /usr/local/bin.
/// Creates socket directory if needed, redirects output unless debug=true.
pub async fn spawn_virtiofsd_async(config: &VirtiofsConfig) -> Result<tokio::process::Child> {
    // Validate configuration
    validate_virtiofsd_config(config)?;

    // Try common virtiofsd binary locations
    let virtiofsd_paths = [
        "/usr/libexec/virtiofsd",
        "/usr/bin/virtiofsd",
        "/usr/local/bin/virtiofsd",
        "/usr/lib/virtiofsd",
    ];

    let virtiofsd_binary = virtiofsd_paths
        .iter()
        .find(|path| std::path::Path::new(path).exists())
        .ok_or_else(|| {
            eyre!(
                "virtiofsd binary not found. Searched paths: {}. Please install virtiofsd.",
                virtiofsd_paths.join(", ")
            )
        })?;

    // Check if virtiofsd supports --readonly flag
    let supports_readonly = virtiofsd_supports_readonly(virtiofsd_binary).await;
    debug!(
        "virtiofsd at {} supports --readonly: {}",
        virtiofsd_binary, supports_readonly
    );

    let mut cmd = tokio::process::Command::new(virtiofsd_binary);
    // SAFETY: This API is safe to call in a forked child.
    #[allow(unsafe_code)]
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::TERM))
                .map_err(Into::into)
        });
    }
    cmd.args([
        "--socket-path",
        config.socket_path.as_str(),
        "--shared-dir",
        config.shared_dir.as_str(),
        // Ensure we don't hit fd exhaustion
        "--cache=never",
        // We always run in a container
        "--sandbox=none",
    ]);

    // Only add --readonly if requested and supported
    if config.readonly && supports_readonly {
        cmd.arg("--readonly");
    }

    // https://gitlab.com/virtio-fs/virtiofsd/-/issues/17 - this is the new default,
    // but we want to be compatible with older virtiofsd too.
    cmd.arg("--inode-file-handles=fallback");

    // Configure output redirection
    if let Some(log_file) = &config.log_file {
        // Create/open log file for both stdout and stderr
        let tokio_file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)
            .await
            .with_context(|| format!("Failed to open virtiofsd log file: {}", log_file))?;

        let log_file_handle = tokio_file.into_std().await;

        // Clone for stderr
        let stderr_handle = log_file_handle
            .try_clone()
            .with_context(|| "Failed to clone log file handle for stderr")?;

        cmd.stdout(std::process::Stdio::from(log_file_handle));
        cmd.stderr(std::process::Stdio::from(stderr_handle));

        debug!("virtiofsd output will be logged to: {}", log_file);
    } else {
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
    }

    let child = cmd.spawn().with_context(|| {
        format!(
            "Failed to spawn virtiofsd. Binary: {}, Socket: {}, Shared dir: {}",
            virtiofsd_binary, config.socket_path, config.shared_dir
        )
    })?;

    debug!(
        "Spawned virtiofsd: binary={}, socket={}, shared_dir={}, debug={}, log_file={:?}",
        virtiofsd_binary, config.socket_path, config.shared_dir, config.debug, config.log_file
    );

    Ok(child)
}

/// Validate virtiofsd configuration.
/// Checks shared directory exists/readable, socket path valid,
/// and cache/sandbox modes are valid values.
pub fn validate_virtiofsd_config(config: &VirtiofsConfig) -> Result<()> {
    // Validate shared directory
    let shared_path = std::path::Path::new(&config.shared_dir);
    if !shared_path.exists() {
        return Err(eyre!(
            "Virtiofsd shared directory does not exist: {}",
            config.shared_dir
        ));
    }

    if !shared_path.is_dir() {
        return Err(eyre!(
            "Virtiofsd shared directory is not a directory: {}",
            config.shared_dir
        ));
    }

    // Check if directory is readable
    match std::fs::read_dir(shared_path) {
        Ok(_) => {}
        Err(e) => {
            return Err(eyre!(
                "Cannot read virtiofsd shared directory {}: {}",
                config.shared_dir,
                e
            ));
        }
    }

    // Validate socket path
    if config.socket_path.as_str().is_empty() {
        return Err(eyre!("Virtiofsd socket path cannot be empty"));
    }

    let socket_path = std::path::Path::new(&config.socket_path);
    if let Some(socket_dir) = socket_path.parent() {
        if !socket_dir.exists() {
            std::fs::create_dir_all(socket_dir).with_context(|| {
                format!(
                    "Failed to create socket directory: {}",
                    socket_dir.display()
                )
            })?;
        }
    }

    Ok(())
}
