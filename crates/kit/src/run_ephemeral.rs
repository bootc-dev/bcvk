//! Ephemeral VM execution using hybrid container-VM approach.
//!
//! This module implements a sophisticated architecture for running container images as
//! ephemeral VMs by orchestrating a multi-stage execution flow through privileged
//! containers, namespace isolation, and VirtioFS filesystem sharing.
//!
//! # Architecture Overview
//!
//! The system uses a "hybrid container-VM" approach that runs QEMU inside privileged
//! Podman containers with KVM access. This combines container isolation with full
//! kernel VM capabilities.
//!
//! ## Execution Flow
//!
//! The execution follows this chain:
//! 1. **Host Process**: `bcvk run-ephemeral` invoked on host
//! 2. **Container Launch**: Podman privileged container with KVM and host mounts
//! 3. **Namespace Setup**: bwrap creates isolated namespace with hybrid rootfs  
//! 4. **Binary Re-execution**: Same binary re-executes with `container-entrypoint`
//! 5. **VM Launch**: QEMU starts with VirtioFS root and additional mounts
//!
//! ## Key Components
//!
//! ### Phase 1: Container Setup (`run_qemu_in_container`)
//! - Runs on the host system
//! - Serializes CLI options to JSON via `BCK_CONFIG` environment variable
//! - Mounts critical resources into container:
//!   - `/run/selfexe`: The bcvk binary itself (for re-execution)
//!   - `/run/source-image`: Target container image via `--mount=type=image`
//!   - `/run/hostusr`: Host `/usr` directory (read-only, for QEMU/tools)
//!   - `/var/lib/bcvk/entrypoint`: Embedded entrypoint.sh script
//! - Handles real-time output streaming for `--execute` commands
//!
//! ### Phase 2: Hybrid Rootfs Creation (entrypoint.sh)
//! The entrypoint script creates a hybrid root filesystem at `/run/tmproot`:
//! ```text
//! /run/tmproot/
//! ├── usr/       → bind mount to /run/hostusr (host binaries)
//! ├── bin/       → symlink to usr/bin
//! ├── lib/       → symlink to usr/lib
//! └── [other dirs created empty for container compatibility]
//! ```
//!
//! ### Phase 3: Namespace Isolation (bwrap)
//! Uses bubblewrap to create isolated namespace:
//! - New mount namespace with `/run/tmproot` as root
//! - Shared `/run/inner-shared` for virtiofsd socket communication
//! - Proper `/proc`, `/dev`, `/tmp` mounts
//! - Re-executes binary: `bwrap ... -- /run/selfexe container-entrypoint`
//!
//! ### Phase 4: VM Execution (`run_impl`)
//! - Runs inside the container after namespace setup
//! - Extracts kernel/initramfs from container image
//! - Spawns virtiofsd daemons for filesystem sharing:
//!   - Main daemon: shares `/run/source-image` as VM root
//!   - Additional daemons: one per host mount (`--bind`/`--ro-bind`)
//! - Generates systemd `.mount` units for virtiofs mounts
//! - Configures and launches QEMU with VirtioFS root
//!
//! ## VirtioFS Architecture
//!
//! The system uses VirtioFS for high-performance filesystem sharing:
//! - **Root FS**: Container image mounted via main virtiofsd at `/run/inner-shared/virtiofs.sock`
//! - **Host Mounts**: Separate virtiofsd per mount at `/run/inner-shared/virtiofs-<name>.sock`
//! - **VM Access**: Mounts appear at `/run/virtiofs-mnt-<name>` via systemd units
//!
//! ## Command Execution (`--execute`)
//!
//! For running commands inside the VM:
//! 1. Creates systemd services (`bootc-execute.service`, `bootc-execute-finish.service`)
//! 2. Uses VirtioSerial devices for output (`execute`) and status (`executestatus`)
//! 3. Streams output in real-time via monitoring thread on host
//! 4. Captures exit codes via systemd service status
//!
//! ## Security Model
//!
//! - **Privileged Container**: Required for KVM and namespace operations
//! - **Read-only Host Access**: Host `/usr` mounted read-only
//! - **SELinux**: Disabled within container only (`--security-opt=label=disable`)
//! - **Network Isolation**: Default "none" unless explicitly configured
//! - **VirtioFS Sandboxing**: Relies on VM isolation for security
//!
//! ## Configuration Passing
//!
//! All CLI options are preserved through the execution chain via JSON serialization:
//! - Host serializes `RunEphemeralOpts` to `BCK_CONFIG` environment variable
//! - Container entrypoint deserializes and re-applies all settings
//! - Ensures perfect fidelity of user options across process boundaries

use std::fs::File;
use std::io::{BufWriter, Seek, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use bootc_utils::CommandRunExt;
use camino::{Utf8Path, Utf8PathBuf};
use clap::Parser;
use color_eyre::eyre::{eyre, Context};
use color_eyre::Result;
use rustix::path::Arg;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tracing::{debug, warn};

const ENTRYPOINT: &str = "/var/lib/bcvk/entrypoint";

/// Get default vCPU count (number of available processors, or 2 as fallback)
pub fn default_vcpus() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(2)
}

use crate::qemu;
use crate::{
    boot_progress,
    common_opts::MemoryOpts,
    podman,
    supervisor_status::{StatusWriter, SupervisorState, SupervisorStatus},
    systemd, utils, CONTAINER_STATEDIR,
};

/// Common container lifecycle options for podman commands.
#[derive(Parser, Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommonPodmanOptions {
    #[clap(
        short = 't',
        long = "tty",
        help = "Allocate a pseudo-TTY for container"
    )]
    pub tty: bool,

    #[clap(
        short = 'i',
        long = "interactive",
        help = "Keep STDIN open for container"
    )]
    pub interactive: bool,

    #[clap(short = 'd', long = "detach", help = "Run container in background")]
    pub detach: bool,

    #[clap(long = "rm", help = "Automatically remove container when it exits")]
    pub rm: bool,

    #[clap(long = "name", help = "Assign a name to the container")]
    pub name: Option<String>,

    #[clap(long = "network", help = "Configure the network for the container")]
    pub network: Option<String>,

    #[clap(
        long = "label",
        help = "Add metadata to the container in key=value form"
    )]
    pub label: Vec<String>,

    #[clap(
        long = "env",
        short = 'e',
        help = "Set environment variables in the container (key=value)"
    )]
    pub env: Vec<String>,
}

/// Common VM configuration options for hardware, networking, and features.
#[derive(Parser, Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommonVmOpts {
    #[clap(
        long,
        help = "Instance type (e.g., u1.nano, u1.small, u1.medium). Overrides vcpus/memory if specified."
    )]
    pub itype: Option<crate::instancetypes::InstanceType>,

    #[clap(flatten)]
    pub memory: MemoryOpts,

    #[clap(long, help = "Number of vCPUs (overridden by --itype if specified)")]
    pub vcpus: Option<u32>,

    #[clap(long, help = "Enable console output to terminal for debugging")]
    pub console: bool,

    #[clap(
        long,
        help = "Enable debug mode (drop to shell instead of running QEMU)"
    )]
    pub debug: bool,

    #[clap(
        long = "virtio-serial-out",
        value_name = "NAME:FILE",
        help = "Add virtio-serial device with output to file (format: name:/path/to/file)"
    )]
    pub virtio_serial_out: Vec<String>,

    #[clap(
        long,
        help = "Execute command inside VM via systemd and capture output"
    )]
    pub execute: Vec<String>,

    #[clap(
        long,
        short = 'K',
        help = "Generate SSH keypair and inject via systemd credentials"
    )]
    pub ssh_keygen: bool,
}

impl CommonVmOpts {
    /// Parse memory specification to MB, using instancetype if specified
    pub fn memory_mb(&self) -> color_eyre::Result<u32> {
        if let Some(itype) = self.itype {
            Ok(itype.memory_mb())
        } else {
            crate::utils::parse_memory_to_mb(&self.memory.memory)
        }
    }

    /// Get vCPU count, using instancetype if specified
    pub fn vcpus(&self) -> color_eyre::Result<u32> {
        if let Some(itype) = self.itype {
            Ok(itype.vcpus())
        } else {
            Ok(self.vcpus.unwrap_or_else(default_vcpus))
        }
    }
}

/// Ephemeral VM options: container-style flags, host bind mounts, systemd injection.
#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
pub struct RunEphemeralOpts {
    #[clap(help = "Container image to run as ephemeral VM")]
    pub image: String,

    #[clap(flatten)]
    pub common: CommonVmOpts,

    #[clap(flatten)]
    pub podman: CommonPodmanOptions,

    /// Do not run the default entrypoint directly, but
    /// instead invoke the provided command (e.g. `bash`).
    #[clap(long)]
    pub debug_entrypoint: Option<String>,

    #[clap(
        long = "bind",
        value_name = "HOST_PATH[:NAME]",
        help = "Bind mount host directory (RW) at /run/virtiofs-mnt-<name>"
    )]
    pub bind_mounts: Vec<String>,

    #[clap(
        long = "ro-bind",
        value_name = "HOST_PATH[:NAME]",
        help = "Bind mount host directory (RO) at /run/virtiofs-mnt-<name>"
    )]
    pub ro_bind_mounts: Vec<String>,

    #[clap(
        long = "systemd-units",
        help = "Directory with systemd units to inject (expects system/ subdirectory)"
    )]
    pub systemd_units_dir: Option<String>,

    #[clap(
        long = "bind-storage-ro",
        help = "Mount host container storage (RO) at /run/virtiofs-mnt-hoststorage"
    )]
    pub bind_storage_ro: bool,

    #[clap(long, help = "Allocate a swap device of the provided size")]
    pub add_swap: Option<String>,

    #[clap(
        long = "mount-disk-file",
        value_name = "FILE[:NAME]",
        help = "Mount disk file as virtio-blk device at /dev/disk/by-id/virtio-<name>"
    )]
    pub mount_disk_files: Vec<String>,

    #[clap(long = "karg", help = "Additional kernel command line arguments")]
    pub kernel_args: Vec<String>,

    /// Host DNS servers (read on host, configured via podman --dns flags)
    /// Not a CLI option - populated automatically from host's /etc/resolv.conf
    #[clap(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_dns_servers: Option<Vec<String>>,
}

/// Parse DNS servers from resolv.conf format content
fn parse_resolv_conf(content: &str) -> Vec<String> {
    let mut dns_servers = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        // Parse lines like "nameserver 8.8.8.8" or "nameserver 2001:4860:4860::8888"
        if let Some(server) = line.strip_prefix("nameserver ") {
            let server = server.trim();
            if !server.is_empty() {
                dns_servers.push(server.to_string());
            }
        }
    }
    dns_servers
}

/// Read DNS servers from host's resolv.conf
/// Returns a vector of DNS server IP addresses, or None if unable to read/parse
///
/// For systemd-resolved systems, reads from /run/systemd/resolve/resolv.conf
/// which contains actual upstream DNS servers, not the stub resolver (127.0.0.53).
/// Falls back to /etc/resolv.conf for non-systemd-resolved systems.
fn read_host_dns_servers() -> Option<Vec<String>> {
    // Try systemd-resolved's upstream DNS file first
    // This avoids reading 127.0.0.53 (stub resolver) from /etc/resolv.conf
    let paths = [
        "/run/systemd/resolve/resolv.conf", // systemd-resolved upstream servers
        "/etc/resolv.conf",                 // traditional or fallback
    ];

    for path in &paths {
        match std::fs::read_to_string(path) {
            Ok(content) => {
                let dns_servers = parse_resolv_conf(&content);

                // Filter out localhost and link-local addresses only
                // Private network addresses (10.x, 172.16-31.x, 192.168.x, fc00::/7) are allowed
                // because they may be reachable from the container/VM (e.g., VPN DNS servers).
                let filtered_servers: Vec<String> = dns_servers
                    .into_iter()
                    .filter(|s| {
                        // Try parsing as IPv4 first
                        if let Ok(ip) = s.parse::<std::net::Ipv4Addr>() {
                            // Reject loopback and link-local addresses only
                            !ip.is_loopback() && !ip.is_link_local()
                        } else if let Ok(ip) = s.parse::<std::net::Ipv6Addr>() {
                            // Reject loopback (::1), link-local (fe80::/10), and multicast
                            !ip.is_loopback()
                                && !ip.is_multicast()
                                && !(ip.segments()[0] & 0xffc0 == 0xfe80) // link-local fe80::/10
                        } else {
                            false // Reject invalid addresses
                        }
                    })
                    .collect();

                if !filtered_servers.is_empty() {
                    debug!("Found DNS servers from {}: {:?}", path, filtered_servers);
                    return Some(filtered_servers);
                } else {
                    debug!("No usable DNS servers in {}, trying next", path);
                }
            }
            Err(e) => {
                debug!("Failed to read {}: {}, trying next", path, e);
            }
        }
    }

    debug!("No DNS servers found in any resolv.conf file");
    None
}

/// Launch privileged container with QEMU+KVM for ephemeral VM, spawning as subprocess.
/// Returns the container ID instead of executing the command.
pub fn run_detached(opts: RunEphemeralOpts) -> Result<String> {
    let (mut cmd, temp_dir) = prepare_run_command_with_temp(opts)?;

    // Leak the tempdir to keep it alive for the entire container lifetime
    std::mem::forget(temp_dir);

    debug!("Podman command: {:?}", cmd);
    let output = cmd.output().context("Failed to execute podman command")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(color_eyre::eyre::eyre!("Podman command failed: {}", stderr));
    }

    // Return the container ID from stdout
    let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(container_id)
}

/// Launch privileged container with QEMU+KVM for ephemeral VM.
pub fn run(opts: RunEphemeralOpts) -> Result<()> {
    let (mut cmd, _temp_dir) = prepare_run_command_with_temp(opts)?;
    // Keep _temp_dir alive until exec replaces our process
    // At this point our process is replaced by `podman`, we are just a wrapper for creating
    // a container image and nothing else lives past that event.
    return Err(cmd.exec()).context("execve");
}

fn prepare_run_command_with_temp(
    opts: RunEphemeralOpts,
) -> Result<(std::process::Command, tempfile::TempDir)> {
    debug!("Running QEMU inside hybrid container for {}", opts.image);

    let script = include_str!("../scripts/entrypoint.sh");

    let td = tempfile::tempdir()?;
    let td_path = td.path().to_str().unwrap();

    let entrypoint_path = &format!("{}/entrypoint", td_path);
    {
        let f = File::create(entrypoint_path)?;
        let mut f = BufWriter::new(f);
        f.write_all(script.as_bytes())?;
        use std::{fs::Permissions, os::unix::fs::PermissionsExt};
        let f = f.into_inner()?;
        let perms = Permissions::from_mode(0o755);
        f.set_permissions(perms)?;
    }

    let self_exe = std::env::current_exe()?;
    let self_exe = self_exe.as_str()?;

    // Process disk files and create them if needed
    let processed_disk_files = process_disk_files(&opts.mount_disk_files, &opts.image)?;

    // Parse mount arguments (both bind and ro-bind)
    let mut host_mounts = Vec::new();

    // Add container storage mount if requested
    if opts.bind_storage_ro {
        let storage_path = utils::detect_container_storage_path().context(
            "Failed to detect container storage path. Use --ro-bind to specify manually.",
        )?;
        utils::validate_container_storage_path(&storage_path)
            .context("Container storage validation failed")?;

        debug!(
            "Adding container storage from {} as hoststorage mount",
            storage_path
        );
        host_mounts.push((storage_path.to_string(), "hoststorage".to_string(), true));
        // true = read-only
    }

    // Parse writable bind mounts
    for mount_spec in &opts.bind_mounts {
        let (host_path, mount_name) = if let Some((path, name)) = mount_spec.split_once(':') {
            (path.to_string(), name.to_string())
        } else {
            let path = mount_spec.clone();
            let name = Utf8Path::new(&path)
                .file_name()
                .unwrap_or("mount")
                .to_string();
            (path, name)
        };
        host_mounts.push((host_path, mount_name, false)); // false = writable
    }

    // Parse read-only bind mounts
    for mount_spec in &opts.ro_bind_mounts {
        let (host_path, mount_name) = if let Some((path, name)) = mount_spec.split_once(':') {
            (path.to_string(), name.to_string())
        } else {
            let path = mount_spec.clone();
            let name = Utf8Path::new(&path)
                .file_name()
                .unwrap_or("mount")
                .to_string();
            (path, name)
        };
        host_mounts.push((host_path, mount_name, true)); // true = read-only
    }

    // Run the container with the setup script
    let mut cmd = Command::new("podman");
    cmd.arg("run");
    // We don't do pulling because then we'd have to propagate all the authfile
    // and status output for that in the general case.
    cmd.arg("--pull=never");
    // We always have a label
    cmd.arg("--label=bcvk.ephemeral=1");
    for label in opts.podman.label.iter() {
        cmd.arg(format!("--label={label}"));
    }

    // We always want this to be a tmpfs on general principle
    // to match the running system. But also, apparently creating
    // unix domain sockets on fuse-overlayfs is buggy in some
    // circumstances.
    cmd.arg("--mount=type=tmpfs,target=/run");

    // Propagate all podman arguments
    if let Some(ref name) = opts.podman.name {
        cmd.args(["--name", name]);
    }
    // Note that (unlike the libvirt flow) we rely on the default bridge network to avoid
    // port conflicts
    if let Some(network) = opts.podman.network.as_deref() {
        cmd.args(["--network", network]);
    }
    if opts.podman.rm {
        cmd.arg("--rm");
    }
    if opts.podman.tty {
        cmd.arg("-t");
    }
    if opts.podman.interactive {
        cmd.arg("-i");
    }
    if opts.podman.detach {
        cmd.arg("-d");
    }
    for env in opts.podman.env.iter() {
        cmd.arg(format!("--env={env}"));
    }

    let vhost_dev = Utf8Path::new(qemu::VHOST_VSOCK)
        .try_exists()?
        .then(|| format!("--device={}", qemu::VHOST_VSOCK));

    cmd.args([
        // Needed to create nested containers (mountns, etc). Note when running
        // with userns (podman unpriv default) this is totally safe. TODO:
        // Default to enabling userns when running rootful.
        "--cap-add=all",
        // We mount the host /usr (though just *read-only*) but to do that we need to
        // disable default SELinux confinement
        "--security-opt=label=disable",
        // Also needed for nested containers
        "--security-opt=seccomp=unconfined",
        "--security-opt=unmask=/proc/*",
        // This is a general hardening thing to do when running privileged
        "-v",
        "/sys:/sys:ro",
        // Ensure we can create large files on the host and not in the overlay
        "-v",
        "/var/tmp:/var/tmp",
        "--device=/dev/kvm",
    ]);
    cmd.args(vhost_dev);
    cmd.args([
        "-v",
        // The core way things work here is we run the host as a nested container
        // inside an outer container. The rest of /run/tmproot will be populated
        // in the entrypoint script, but we just grab the host's `/usr`.
        // (We don't want all of `/` as that would scope in a lot more)
        "/usr:/run/tmproot/usr:ro",
        "-v",
        &format!("{}:{}", entrypoint_path, ENTRYPOINT),
        "-v",
        &format!("{self_exe}:/run/selfexe:ro"),
        // Since we run as init by default
        "--stop-signal=SIGKILL",
        // And bind mount in the pristine image (without any mounts on top)
        // that we'll use as a mount source for virtiofs. Mount as rw for testing.
        &format!(
            "--mount=type=image,source={},target=/run/source-image,rw=true",
            opts.image.as_str()
        ),
    ]);

    // Add host directory mounts to the container
    for (host_path, mount_name, is_readonly) in &host_mounts {
        let mount_spec = if *is_readonly {
            format!("{}:/run/host-mounts/{}:ro", host_path, mount_name)
        } else {
            format!("{}:/run/host-mounts/{}", host_path, mount_name)
        };
        cmd.args(["-v", &mount_spec]);
    }

    // Mount disk files into the container
    for (disk_file, disk_name, _format) in &processed_disk_files {
        let container_disk_path = format!("/run/disk-files/{}", disk_name);
        cmd.args(["-v", &format!("{}:{}:rw", disk_file, container_disk_path)]);
    }

    // Mount systemd units directory if specified
    if let Some(ref units_dir) = opts.systemd_units_dir {
        cmd.args(["-v", &format!("{}:/run/systemd-units:ro", units_dir)]);
    }

    // Read host DNS servers and configure them via podman --dns flags
    // This fixes DNS resolution issues when QEMU runs inside containers.
    // QEMU's slirp reads /etc/resolv.conf from the container's network namespace,
    // which would otherwise contain unreachable bridge DNS servers (e.g., 169.254.1.1).
    // Using --dns properly configures /etc/resolv.conf in the container.
    let host_dns_servers = read_host_dns_servers();

    if let Some(ref dns) = host_dns_servers {
        debug!("Using DNS servers for ephemeral VM: {:?}", dns);
        // Configure DNS servers for the container using --dns flags
        // This properly sets up /etc/resolv.conf in the container's network namespace
        for server in dns {
            cmd.args(["--dns", server]);
        }
    }

    // Pass configuration as JSON via BCK_CONFIG environment variable
    // Include host DNS servers in the config so they're available inside the container
    let mut opts_with_dns = opts.clone();
    opts_with_dns.host_dns_servers = host_dns_servers;
    let config = serde_json::to_string(&opts_with_dns).unwrap();
    cmd.args(["-e", &format!("BCK_CONFIG={config}")]);

    // Handle --execute output files and virtio-serial devices
    let mut all_serial_devices = opts.common.virtio_serial_out.clone();
    if !opts.common.execute.is_empty() {
        // Add virtio-serial devices for execute output and status
        // These will be created inside the container at /run/execute-output/
        all_serial_devices.push("execute:/run/execute-output/execute-output.txt".to_string());
        all_serial_devices.push("executestatus:/run/execute-output/execute-status.txt".to_string());
    }

    // Pass disk files as environment variable
    if !processed_disk_files.is_empty() {
        let disk_specs = processed_disk_files
            .iter()
            .map(|(_, disk_name, format)| {
                format!(
                    "/run/disk-files/{}:{}:{}",
                    disk_name,
                    disk_name,
                    format.as_str()
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        cmd.args(["-e", &format!("BOOTC_DISK_FILES={}", disk_specs)]);
    }

    let entrypoint = opts.debug_entrypoint.as_deref().unwrap_or(ENTRYPOINT);
    cmd.args([&opts.image, entrypoint]);

    Ok((cmd, td))
}

/// Process --mount-disk-file specs: parse file:name format, create sparse files if needed (2x image size),
/// validate only regular files, convert to absolute paths.
pub(crate) fn process_disk_files(
    disk_specs: &[String],
    image: &str,
) -> Result<Vec<(Utf8PathBuf, String, crate::to_disk::Format)>> {
    use std::fs::File;

    let mut processed_disks = Vec::new();

    if disk_specs.is_empty() {
        return Ok(processed_disks);
    }

    // Get image size for auto-sizing new disk files (2x the image size)
    let image_size = podman::get_image_size(image)?;
    // Use minimum 4GB or 2x image size, whichever is larger
    let disk_size = std::cmp::max(image_size * 2, 4u64 * 1024 * 1024 * 1024);

    for disk_spec in disk_specs {
        let (disk_file, disk_name, format) = if let Some((file, rest)) = disk_spec.split_once(':') {
            if let Some((name, format_str)) = rest.split_once(':') {
                let format = match format_str {
                    "raw" => crate::to_disk::Format::Raw,
                    "qcow2" => crate::to_disk::Format::Qcow2,
                    _ => return Err(eyre!("Unsupported disk format: {}", format_str)),
                };
                (file.to_string(), name.to_string(), format)
            } else {
                // Auto-detect format from file extension if not explicitly provided
                let format = if file.ends_with(".qcow2") {
                    crate::to_disk::Format::Qcow2
                } else {
                    crate::to_disk::Format::Raw
                };
                (file.to_string(), rest.to_string(), format)
            }
        } else {
            // Auto-detect format from file extension if not explicitly provided
            let format = if disk_spec.ends_with(".qcow2") {
                crate::to_disk::Format::Qcow2
            } else {
                crate::to_disk::Format::Raw
            };
            (disk_spec.clone(), "output".to_string(), format)
        };

        let disk_path = Utf8Path::new(&disk_file);

        // Security check: only accept regular files
        if disk_path.exists() {
            let metadata = disk_path
                .metadata()
                .with_context(|| format!("Failed to get metadata for disk file: {}", disk_file))?;

            if !metadata.is_file() {
                return Err(eyre!(
                    "Disk file must be a regular file, not a directory or block device: {}",
                    disk_file
                ));
            }
        } else {
            // Create sparse disk image file
            debug!(
                "Creating new disk file {} (size: {} bytes)",
                disk_file, disk_size
            );
            let file = File::create(&disk_path)
                .with_context(|| format!("Failed to create disk file: {}", disk_file))?;

            file.set_len(disk_size)
                .with_context(|| format!("Failed to set size for disk file: {}", disk_file))?;

            debug!("Created sparse disk image: {}", disk_file);
        }

        // Convert relative paths to absolute paths for QEMU
        let absolute_disk_file = if disk_path.is_absolute() {
            disk_file.into()
        } else {
            let p = disk_path.canonicalize()?;
            Utf8PathBuf::try_from(p)?
        };

        debug!(
            "Processed disk file: path={}, name={}, format={}",
            absolute_disk_file, disk_name, format
        );
        processed_disks.push((absolute_disk_file, disk_name, format));
    }

    Ok(processed_disks)
}

/// Copy systemd units from /run/systemd-units/system/ to container image /etc/systemd/system/.
/// Auto-enables .mount units in local-fs.target.wants/, preserves default.target.wants/ symlinks.
fn inject_systemd_units() -> Result<()> {
    use std::fs;

    debug!("Injecting systemd units from /run/systemd-units");

    let source_units = Utf8Path::new("/run/systemd-units/system");
    if !source_units.exists() {
        debug!("No systemd units to inject at {}", source_units);
        return Ok(());
    }
    let target_units = "/run/source-image/etc/systemd/system";

    // Create target directories
    fs::create_dir_all(target_units)?;
    fs::create_dir_all(&format!("{}/default.target.wants", target_units))?;
    fs::create_dir_all(&format!("{}/local-fs.target.wants", target_units))?;

    // Copy all .service and .mount files
    for entry in fs::read_dir(source_units)? {
        let entry = entry?;
        let path = entry.path();
        let extension = path.extension().map(|ext| ext.to_string_lossy());
        if matches!(extension.as_deref(), Some("service") | Some("mount")) {
            let filename = path.file_name().unwrap().to_string_lossy();
            let target_path = format!("{}/{}", target_units, filename);
            fs::copy(&path, &target_path)?;
            debug!("Copied systemd unit: {}", filename);

            // Create symlinks for mount units to enable them
            if extension.as_deref() == Some("mount") {
                let wants_dir = format!("{}/local-fs.target.wants", target_units);
                let symlink_path = format!("{}/{}", wants_dir, filename);
                let relative_target = format!("../{}", filename);
                std::os::unix::fs::symlink(&relative_target, &symlink_path).ok();
                debug!("Enabled mount unit: {}", filename);
            }
        }
    }

    // Copy wants directory if it exists
    let source_wants = "/run/systemd-units/system/default.target.wants";
    let target_wants = &format!("{}/default.target.wants", target_units);

    if Utf8Path::new(source_wants).exists() {
        for entry in fs::read_dir(source_wants)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_symlink() || path.is_file() {
                let filename = path.file_name().unwrap().to_string_lossy();
                let target_path = format!("{}/{}", target_wants, filename);

                if path.is_symlink() {
                    let link_target = fs::read_link(&path)?;
                    let _ = fs::remove_file(&target_path); // Remove if exists
                    std::os::unix::fs::symlink(link_target, &target_path)?;
                } else {
                    fs::copy(&path, &target_path)?;
                }
                debug!("Copied systemd wants link: {}", filename);
            }
        }
    }

    debug!("Systemd unit injection complete");
    Ok(())
}

/// Parse exit code from systemd service status output
fn parse_service_exit_code(status_content: &str) -> Result<i32> {
    for line in status_content.lines() {
        if let Some(codeval) = line.strip_prefix("ExecMainStatus=") {
            let exit_code: i32 = codeval.parse().context("Parsing ExecMainStatus")?;
            return Ok(exit_code);
        }
    }
    // If no exit code found, assume success
    Ok(0)
}

/// Check for required binaries in the target container image
///
/// These binaries must be present in the container image being run as an ephemeral VM.
fn check_required_container_binaries() -> Result<()> {
    // We use systemctl in a few places. objcopy is for UKI extraction.
    let required_binaries = ["systemctl", "objcopy"];

    let mut missing = Vec::new();

    for binary in &required_binaries {
        if which::which(binary).is_err() {
            missing.push(format!("Missing required executable: {}", binary));
        }
    }

    if !missing.is_empty() {
        return Err(eyre!("{}", missing.join("\n")));
    }

    debug!("All required container binaries found");
    Ok(())
}

/// VM execution inside container: extracts kernel/initramfs, starts virtiofsd processes,
/// generates systemd mount units, sets up command execution, launches QEMU.
pub(crate) async fn run_impl(opts: RunEphemeralOpts) -> Result<()> {
    use crate::qemu;
    use std::fs;

    debug!("Running QEMU implementation inside container");

    // Check for required binaries in the target container image early
    check_required_container_binaries()?;

    // Initialize status writer for supervisor monitoring
    let status_writer = StatusWriter::new("/run/supervisor-status.json");
    status_writer.update_state(SupervisorState::WaitingForSystemd)?;

    // Check systemd version from the container image
    let systemd_version = {
        Some(std::env::var("SYSTEMD_VERSION")?)
            .filter(|v| !v.is_empty())
            .as_deref()
            .map(systemd::SystemdVersion::from_version_output)
            .transpose()?
    };
    debug!("Container image systemd version: {systemd_version:?}");

    // Check if we need to handle cloud-init
    let cloudinit = {
        Command::new("systemctl")
            .args([
                "--root=/run/source-image",
                "is-enabled",
                "cloud-init.target",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
            .success()
    };
    tracing::debug!("Target image has cloud-init: {cloudinit}");

    // Verify KVM access
    if !Utf8Path::new("/dev/kvm").exists() || !fs::File::open("/dev/kvm").is_ok() {
        return Err(eyre!("KVM device not accessible"));
    }

    // Create QEMU mount points
    fs::create_dir_all("/run/qemu")?;

    // Find kernel and initramfs in /usr/lib/modules/
    let modules_dir = Utf8Path::new("/run/source-image/usr/lib/modules");
    let mut uki_file: Option<Utf8PathBuf> = None;
    let mut vmlinuz_path: Option<Utf8PathBuf> = None;
    let mut initramfs_path: Option<Utf8PathBuf> = None;

    let entries = fs::read_dir(modules_dir)
        .with_context(|| format!("Failed to read kernel modules directory at {}. This container image may not be a valid bootc image.", modules_dir))?;

    for entry in entries {
        let entry = entry?;
        let path = Utf8PathBuf::from_path_buf(entry.path())
            .map_err(|p| eyre!("Path is not valid UTF-8: {}", p.display()))?;

        // Check for UKI (.efi file)
        if path.is_file() && path.extension() == Some("efi") {
            debug!("Found UKI file: {:?}", path);
            uki_file = Some(path);
            break;
        }

        // Check for traditional kernel in subdirectories
        if path.is_dir() {
            let vmlinuz = path.join("vmlinuz");
            let initramfs = path.join("initramfs.img");
            if vmlinuz.exists() && initramfs.exists() {
                debug!("Found kernel at: {:?}", vmlinuz);
                vmlinuz_path = Some(vmlinuz);
                initramfs_path = Some(initramfs);
                break;
            }
        }
    }

    let kernel_mount = "/run/qemu/kernel";
    let initramfs_mount = "/run/qemu/initramfs";

    // Extract from UKI if found, otherwise use traditional kernel
    if let Some(uki_path) = uki_file {
        debug!("Extracting kernel and initramfs from UKI: {:?}", uki_path);

        // Extract .linux section (kernel) from UKI
        Command::new("objcopy")
            .args([
                "--dump-section",
                &format!(".linux={}", kernel_mount),
                uki_path.as_str(),
            ])
            .run()
            .map_err(|e| eyre!("Failed to extract kernel from UKI: {e}"))?;
        debug!("Extracted kernel from UKI to {}", kernel_mount);

        // Extract .initrd section (initramfs) from UKI
        Command::new("objcopy")
            .args([
                "--dump-section",
                &format!(".initrd={}", initramfs_mount),
                uki_path.as_str(),
            ])
            .run()
            .map_err(|e| eyre!("Failed to extract initramfs from UKI: {e}"))?;
        debug!("Extracted initramfs from UKI to {}", initramfs_mount);
    } else {
        let vmlinuz_path = vmlinuz_path
            .ok_or_else(|| eyre!("No kernel found in /run/source-image/usr/lib/modules"))?;
        let source_initramfs_path = initramfs_path
            .ok_or_else(|| eyre!("No initramfs found in /run/source-image/usr/lib/modules"))?;

        fs::File::create(&kernel_mount)?;

        // Bind mount kernel (read-only is fine)
        Command::new("mount")
            .args(["--bind", "-o", "ro", vmlinuz_path.as_str(), &kernel_mount])
            .run()
            .map_err(|e| eyre!("Failed to bind mount kernel: {e}"))?;

        // Copy initramfs so we can append to it
        fs::copy(&source_initramfs_path, &initramfs_mount)
            .map_err(|e| eyre!("Failed to copy initramfs: {e}"))?;
    }

    // Append bcvk units to initramfs
    // This includes:
    // - /etc overlay and /var ephemeral services (run in initramfs)
    // - bcvk-copy-units.service (copies journal-stream to /sysroot/etc for systemd <256)
    // - bcvk-journal-stream.service (embedded for systemd <256 compatibility)
    //
    // The Linux kernel's initramfs parser requires uncompressed CPIO archives to start
    // at a 4-byte aligned offset. We add NUL padding before our CPIO data to ensure
    // proper alignment. The kernel skips NUL bytes between archives.
    {
        use std::io::{Seek, SeekFrom, Write};
        let cpio_data = crate::cpio::create_initramfs_units_cpio()
            .map_err(|e| eyre!("Failed to create initramfs CPIO: {e}"))?;
        let mut initramfs_file = fs::OpenOptions::new()
            .append(true)
            .open(&initramfs_mount)
            .map_err(|e| eyre!("Failed to open initramfs for appending: {e}"))?;

        // Get current file size to calculate alignment padding
        let current_size: u64 = initramfs_file
            .seek(SeekFrom::End(0))
            .map_err(|e| eyre!("Failed to get initramfs size: {e}"))?;
        let aligned_size = current_size.next_multiple_of(4);
        let padding_needed = aligned_size - current_size;
        if padding_needed > 0 {
            initramfs_file
                .write_all(&vec![0u8; padding_needed as usize])
                .map_err(|e| eyre!("Failed to write alignment padding: {e}"))?;
        }

        initramfs_file
            .write_all(&cpio_data)
            .map_err(|e| eyre!("Failed to append bcvk units to initramfs: {e}"))?;
        debug!(
            "Appended bcvk units to initramfs ({} bytes padding + {} bytes CPIO)",
            padding_needed,
            cpio_data.len()
        );
    }

    // Process host mounts and prepare virtiofsd instances for each using async manager
    let mut additional_mounts = Vec::new();
    // Collect mount unit credentials to inject via SMBIOS instead of writing to filesystem
    let mut mount_unit_smbios_creds = Vec::new();

    debug!(
        "Checking for host mounts directory: /run/host-mounts exists = {}",
        Utf8Path::new("/run/host-mounts").exists()
    );
    debug!(
        "Checking for systemd units directory: /run/systemd-units exists = {}",
        Utf8Path::new("/run/systemd-units").exists()
    );

    let mut mount_unit_names = Vec::new();
    if Utf8Path::new("/run/host-mounts").exists() {
        for entry in fs::read_dir("/run/host-mounts")? {
            let entry = entry?;
            let mount_name = entry.file_name();
            let mount_name_str = mount_name.to_string_lossy();
            let source_path: Utf8PathBuf = entry.path().try_into()?;
            let mount_path = format!("/run/host-mounts/{}", mount_name_str);

            // Check if this directory is mounted as read-only
            let is_readonly =
                !rustix::fs::access(&mount_path, rustix::fs::Access::WRITE_OK).is_ok();

            let mode = if is_readonly { "ro" } else { "rw" };
            debug!(
                "Setting up virtiofs mount for {} ({})",
                mount_name_str, mode
            );

            // Create virtiofs socket path and tag
            let socket_path = format!("/run/inner-shared/virtiofs-{}.sock", mount_name_str);
            let tag = format!("mount_{}", mount_name_str);

            // Store virtiofsd config to be spawned later by QEMU
            let virtiofsd_config = qemu::VirtiofsConfig {
                socket_path: socket_path.clone().into(),
                shared_dir: source_path,
                debug: false,
                readonly: is_readonly,
                log_file: Some(format!("/run/virtiofsd-{}.log", mount_name_str).into()),
            };
            additional_mounts.push((virtiofsd_config, tag.clone()));

            // Generate mount unit via SMBIOS credentials instead of writing to filesystem
            let mount_point = format!("/run/virtiofs-mnt-{}", mount_name_str);
            let unit_name = crate::credentials::guest_path_to_unit_name(&mount_point);
            let mount_unit_content =
                crate::credentials::generate_virtiofs_mount_unit(&tag, &mount_point, is_readonly);
            let encoded_mount = data_encoding::BASE64.encode(mount_unit_content.as_bytes());

            // Create SMBIOS credential for the mount unit
            let mount_cred = format!(
                "io.systemd.credential.binary:systemd.extra-unit.{unit_name}={encoded_mount}"
            );
            mount_unit_smbios_creds.push(mount_cred);

            // Collect unit name for the local-fs.target dropin
            mount_unit_names.push(unit_name.clone());

            debug!(
                "Generated SMBIOS credential for mount unit: {} ({})",
                unit_name, mode
            );
        }
    }

    // If we have mount units, create a single dropin for local-fs.target
    if !mount_unit_names.is_empty() {
        let wants_list = mount_unit_names.join(" ");
        let dropin_content = format!("[Unit]\nWants={}\n", wants_list);
        let encoded_dropin = data_encoding::BASE64.encode(dropin_content.as_bytes());
        let dropin_cred = format!(
            "io.systemd.credential.binary:systemd.unit-dropin.local-fs.target~bcvk-mounts={encoded_dropin}"
        );
        mount_unit_smbios_creds.push(dropin_cred);
        debug!(
            "Created local-fs.target dropin for {} mount units",
            mount_unit_names.len()
        );
    }

    // Note: /etc overlay and /var ephemeral units are now embedded directly in the
    // initramfs CPIO (see cpio.rs) rather than injected via SMBIOS credentials.
    // This ensures they work on systemd <256 where credential import happens too
    // late for generators to process.

    // Handle --execute: pipes will be created when adding to qemu_config later
    // No need to create files anymore as we're using pipes

    // Create systemd unit to stream journal to virtio-serial device via SMBIOS credential
    let journal_stream_unit = r#"[Unit]
Description=Stream systemd journal to host via virtio-serial
DefaultDependencies=no
After=systemd-journald.service dev-virtio\x2dports-org.bcvk.journal.device
Requires=systemd-journald.service dev-virtio\x2dports-org.bcvk.journal.device

[Service]
Type=simple
ExecStart=/usr/bin/journalctl -f -o short-precise --no-pager
StandardOutput=file:/dev/virtio-ports/org.bcvk.journal
StandardError=file:/dev/virtio-ports/org.bcvk.journal
Restart=always
RestartSec=1s

[Install]
WantedBy=sysinit.target
"#;
    let encoded_journal = data_encoding::BASE64.encode(journal_stream_unit.as_bytes());
    let journal_cred = format!(
        "io.systemd.credential.binary:systemd.extra-unit.bcvk-journal-stream.service={encoded_journal}"
    );
    mount_unit_smbios_creds.push(journal_cred);
    debug!("Generated SMBIOS credential for journal streaming unit");

    // Create dropin for sysinit.target to enable journal streaming
    let journal_dropin = "[Unit]\nWants=bcvk-journal-stream.service\n";
    let encoded_dropin = data_encoding::BASE64.encode(journal_dropin.as_bytes());
    let dropin_cred = format!(
        "io.systemd.credential.binary:systemd.unit-dropin.sysinit.target~bcvk-journal={encoded_dropin}"
    );
    mount_unit_smbios_creds.push(dropin_cred);
    debug!("Created sysinit.target dropin to enable journal streaming unit");

    // Create execute units via SMBIOS credentials if needed
    match opts.common.execute.as_slice() {
        [] => {}
        elts => {
            let mut service_content = format!(
                r#"[Unit]
Description=Execute Script Service
Requires=dev-virtio\x2dports-execute.device
After=dev-virtio\x2dports-execute.device
# Ensure we only run after switch-root in the real root filesystem
ConditionPathExists=!/etc/initrd-release

[Service]
Type=oneshot
RemainAfterExit=yes
StandardOutput=file:/dev/virtio-ports/execute
StandardError=inherit
"#
            );
            for elt in elts {
                service_content.push_str(&format!("ExecStart={elt}\n"));
            }

            let service_finish = r#"[Unit]
Description=Execute Script Service Completion
After=bootc-execute.service
Requires=dev-virtio\x2dports-executestatus.device
After=dev-virtio\x2dports-executestatus.device
# Ensure we only run after switch-root in the real root filesystem
ConditionPathExists=!/etc/initrd-release

[Service]
Type=oneshot
ExecStart=systemctl show bootc-execute
ExecStart=systemctl poweroff
StandardOutput=file:/dev/virtio-ports/executestatus
"#;

            // Inject execute units via SMBIOS credentials
            let encoded_execute = data_encoding::BASE64.encode(service_content.as_bytes());
            let execute_cred = format!(
                "io.systemd.credential.binary:systemd.extra-unit.bootc-execute.service={encoded_execute}"
            );
            mount_unit_smbios_creds.push(execute_cred);

            let encoded_finish = data_encoding::BASE64.encode(service_finish.as_bytes());
            let finish_cred = format!(
                "io.systemd.credential.binary:systemd.extra-unit.bootc-execute-finish.service={encoded_finish}"
            );
            mount_unit_smbios_creds.push(finish_cred);

            // Create dropin for multi-user.target to enable execute services
            // Using multi-user.target instead of default.target ensures these only run
            // after switch-root in the real root filesystem (not in initramfs)
            let execute_dropin =
                "[Unit]\nWants=bootc-execute.service bootc-execute-finish.service\n";
            let encoded_dropin = data_encoding::BASE64.encode(execute_dropin.as_bytes());
            let dropin_cred = format!(
                "io.systemd.credential.binary:systemd.unit-dropin.multi-user.target~bcvk-execute={encoded_dropin}"
            );
            mount_unit_smbios_creds.push(dropin_cred);
            debug!("Generated SMBIOS credentials for execute units");
        }
    }

    // Copy systemd units if provided (for --systemd-units-dir option)
    inject_systemd_units()?;

    // Prepare main virtiofsd config for the source image (will be spawned by QEMU)
    let mut main_virtiofsd_config = qemu::VirtiofsConfig::default();
    main_virtiofsd_config.debug = std::env::var("DEBUG_MODE").is_ok();
    // Always log virtiofsd output for debugging
    main_virtiofsd_config.log_file = Some("/run/virtiofsd.log".into());

    std::fs::create_dir_all(CONTAINER_STATEDIR)?;

    // Configure qemu for direct kernel boot
    debug!("Configuring QEMU for direct kernel boot");
    let mut qemu_config = crate::qemu::QemuConfig::new_direct_boot(
        opts.common.memory_mb()?,
        opts.common.vcpus()?,
        "/run/qemu/kernel".to_string(),
        "/run/qemu/initramfs".to_string(),
        main_virtiofsd_config.socket_path.clone(),
    );

    // Check for BCVK_DEBUG=disable-vsock to force disabling vsock for testing
    let vsock_force_disabled = std::env::var("BCVK_DEBUG").as_deref() == Ok("disable-vsock");
    let vsock_enabled = !vsock_force_disabled && qemu_config.enable_vsock().is_ok();

    // Handle SSH key generation and credential injection
    if opts.common.ssh_keygen {
        let key_pair = crate::ssh::generate_default_keypair()?;
        // Create credential and add to kernel args
        let pubkey = std::fs::read_to_string(key_pair.public_key_path.as_path())?;
        let credential = crate::credentials::smbios_cred_for_root_ssh(&pubkey)?;
        qemu_config.add_smbios_credential(credential);
    }

    // Build kernel command line for direct boot
    let mut kernel_cmdline = [
        // At the core we boot from the mounted container's root,
        "rootfstype=virtiofs",
        "root=rootfs",
        // But read-only. We set up /etc overlay and /var copyup via
        // systemd credentials rather than systemd.volatile=overlay
        // to have more control over individual directories.
        "rootflags=ro",
        // This avoids having journald interact with the rootfs
        // at all, which lessens the I/O traffic for virtiofs
        "systemd.journald.storage=volatile",
        // See https://github.com/bootc-dev/bcvk/issues/22
        "selinux=0",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect::<Vec<_>>();

    if opts.common.console {
        kernel_cmdline.push("console=hvc0".to_string());
    }
    if cloudinit {
        // We don't provide any cloud-init datasource right now,
        // though in the future it would make sense to do so,
        // and switch over our SSH key injection.
        kernel_cmdline.push("ds=iid-datasource-none".to_string());
    }

    kernel_cmdline.extend(opts.kernel_args.clone());
    qemu_config.set_kernel_cmdline(kernel_cmdline);

    // TODO allocate unlinked unnamed file and pass via fd
    let mut tmp_swapfile = None;
    if let Some(size) = opts.add_swap {
        let size = utils::parse_size(&size)?;
        debug!("Allocating swap: {size}");
        let mut tmpf = tempfile::NamedTempFile::new_in("/var/tmp")?;
        tmpf.as_file_mut()
            .set_len(size)
            .context("Allocating swap tempfile")?;
        tmpf.seek(std::io::SeekFrom::Start(0))?;
        let path: &Utf8Path = tmpf.path().try_into().unwrap();

        Command::new("mkswap")
            .args(["-q", path.as_str()])
            .run()
            .map_err(|e| eyre!("{e}"))?;

        qemu_config.add_virtio_blk_device_with_format(
            path.to_owned().into(),
            "swap".into(),
            crate::to_disk::Format::Raw,
        );

        // Create swap unit via SMBIOS credential
        let svc = r#"[Unit]
Description=bcvk ephemeral swap

[Swap]
What=/dev/disk/by-id/virtio-swap
Options=
"#;
        let service_name = r#"dev-disk-by\x2did-virtio\x2dswap.swap"#;
        let encoded_swap = data_encoding::BASE64.encode(svc.as_bytes());
        let swap_cred = format!(
            "io.systemd.credential.binary:systemd.extra-unit.{service_name}={encoded_swap}"
        );
        mount_unit_smbios_creds.push(swap_cred);

        // Create dropin for default.target to enable swap
        let swap_dropin = format!("[Unit]\nWants={service_name}\n");
        let encoded_dropin = data_encoding::BASE64.encode(swap_dropin.as_bytes());
        let dropin_cred = format!(
            "io.systemd.credential.binary:systemd.unit-dropin.default.target~bcvk-swap={encoded_dropin}"
        );
        mount_unit_smbios_creds.push(dropin_cred);
        debug!("Generated SMBIOS credential for swap unit");

        tmp_swapfile = Some(tmpf);
    }

    // Parse disk files from environment variable
    let mut virtio_blk_devices = Vec::new();
    if let Ok(disk_env) = std::env::var("BOOTC_DISK_FILES") {
        debug!("Processing BOOTC_DISK_FILES: {}", disk_env);
        for disk_spec in disk_env.split(',') {
            // Parse disk_file:disk_name:format or disk_file:disk_name (auto-detect format)
            let parts: Vec<&str> = disk_spec.splitn(3, ':').collect();
            if parts.len() >= 2 {
                let format = if parts.len() == 3 {
                    match parts[2] {
                        "qcow2" => crate::to_disk::Format::Qcow2,
                        "raw" => crate::to_disk::Format::Raw,
                        _ => {
                            // Auto-detect from file extension as fallback
                            if parts[0].ends_with(".qcow2") {
                                crate::to_disk::Format::Qcow2
                            } else {
                                crate::to_disk::Format::Raw
                            }
                        }
                    }
                } else {
                    // Auto-detect format from file extension
                    if parts[0].ends_with(".qcow2") {
                        crate::to_disk::Format::Qcow2
                    } else {
                        crate::to_disk::Format::Raw
                    }
                };

                let disk_file = parts[0].to_string();
                let serial = parts[1].to_string();

                // Check if disk file exists and is accessible
                if !Utf8Path::new(&disk_file).exists() {
                    return Err(eyre!(
                        "Disk file does not exist in bwrap namespace: {} (serial: {})",
                        disk_file,
                        serial
                    ));
                }

                debug!(
                    "Adding virtio-blk device: file={}, serial={}, format={:?}",
                    disk_file, serial, format
                );

                virtio_blk_devices.push(crate::qemu::VirtioBlkDevice {
                    disk_file,
                    serial,
                    format,
                });
            }
        }
    }

    qemu_config.set_console(opts.common.console);

    // Add virtio-serial device for journal streaming
    qemu_config.add_virtio_serial_out("org.bcvk.journal", "/run/journal.log".to_string(), false);
    debug!("Added virtio-serial device for journal streaming to /run/journal.log");

    // DNS is configured via podman --dns flags (see prepare_run_command_with_temp)
    // This fixes DNS resolution issues when QEMU runs inside containers.
    // QEMU's slirp reads /etc/resolv.conf from the container's network namespace,
    // and podman properly sets it up using --dns instead of relying on bridge DNS.
    if let Some(ref dns_servers) = opts.host_dns_servers {
        debug!("DNS servers configured for QEMU slirp: {:?}", dns_servers);
    } else {
        warn!("No host DNS servers available, QEMU slirp will use container's resolv.conf which may not work");
    }

    if opts.common.ssh_keygen {
        qemu_config.enable_ssh_access(None); // Use default port 2222
        debug!("Enabled SSH port forwarding: host port 2222 -> guest port 22");

        // We need to extract the public key from the SSH credential to inject it via SMBIOS
        // For now, the credential is already being passed via kernel cmdline
        // TODO: Add proper SMBIOS credential injection if needed
    }

    // Set main virtiofs configuration for root filesystem (will be spawned by QEMU)
    qemu_config.set_main_virtiofs(main_virtiofsd_config.clone());

    // Add additional virtiofs configurations (will be spawned by QEMU)
    for (virtiofs_config, tag) in additional_mounts {
        qemu_config.add_virtiofs(virtiofs_config, &tag);
    }

    let exec_pipes = if !opts.common.execute.is_empty() {
        let execute_pipefd: File = qemu_config.add_virtio_serial_pipe("execute")?.into();
        let status_pipefd: File = qemu_config.add_virtio_serial_pipe("executestatus")?.into();
        Some((execute_pipefd, status_pipefd))
    } else {
        None
    };

    // Add virtio-blk devices
    for blk_device in virtio_blk_devices {
        qemu_config.add_virtio_blk_device_with_format(
            blk_device.disk_file,
            blk_device.serial,
            blk_device.format,
        );
    }

    let status_writer_clone = StatusWriter::new("/run/supervisor-status.json");

    // Only enable systemd notification debugging if the systemd version supports it
    // and the host has vsock enabled
    let systemd_has_vmm_notify = systemd_version
        .map(|v| v.has_vmm_notify())
        .unwrap_or_default();
    let mut status_writer_task = None;
    if vsock_enabled && systemd_has_vmm_notify {
        let (piper, pipew) = rustix::pipe::pipe()?;
        qemu_config.systemd_notify = Some(File::from(pipew));
        debug!("Enabling systemd notification debugging");

        // Run this in the background
        status_writer_task = Some(tokio::task::spawn(boot_progress::monitor_boot_progress(
            File::from(piper),
            status_writer_clone,
        )));
    } else {
        debug!("systemd version does not support vmm.notify_socket",);
        // For older systemd versions, write an unknown state
        status_writer.update(SupervisorStatus {
            running: true,
            ..Default::default()
        })?;
    };

    // Add all SMBIOS credentials for mount units, journal, and execute services
    let cred_count = mount_unit_smbios_creds.len();
    for cred in mount_unit_smbios_creds {
        qemu_config.add_smbios_credential(cred);
    }
    debug!("Added {} SMBIOS credentials to QEMU config", cred_count);

    debug!("Starting QEMU with systemd debugging enabled");

    // Spawn QEMU with all virtiofsd processes handled internally
    let mut qemu = match crate::qemu::RunningQemu::spawn(qemu_config).await {
        Ok(r) => r,
        Err(e) => {
            tracing::trace!("Aborting status writer");
            if let Some(writer) = status_writer_task {
                writer.abort();
            }
            return Err(e);
        }
    };

    // Handle execute command output streaming if needed
    if let Some((exec_pipefd, status_pipefd)) = exec_pipes {
        tracing::debug!("Starting execute output streaming with pipes");
        let output_copier = async move {
            let fd = tokio::fs::File::from(exec_pipefd);
            let mut bufr = tokio::io::BufReader::new(fd);
            let mut stdout = tokio::io::stdout();
            let result = tokio::io::copy(&mut bufr, &mut stdout).await;
            tracing::debug!("Output copy result: {:?}", result);
            result
        };
        let mut status_reader = tokio::io::BufReader::new(tokio::fs::File::from(status_pipefd));
        let mut status = String::new();
        let status_reader = status_reader.read_to_string(&mut status);

        // And wait for all tasks
        let (qemu, output_copier, execstatus) =
            tokio::join!(qemu.wait(), output_copier, status_reader);
        // Do check for errors from reading from the execstatus pipe
        let _ = execstatus.context("Reading execstatus")?;

        // Discard errors from qemu and the output copier
        tracing::debug!("qemu exit status: {qemu:?}");
        tracing::debug!("output copy: {output_copier:?}");

        // Parse exit code from systemd service status
        let exit_code = parse_service_exit_code(&status)?;
        if exit_code != 0 {
            return Err(eyre!(
                "Execute command failed with exit code: {}",
                exit_code
            ));
        }
    } else {
        // Wait for QEMU to complete
        tracing::debug!("Waiting for qemu exit");
        let exit_status = qemu.wait().await?;
        if !exit_status.success() {
            return Err(eyre!("QEMU exited with non-zero status: {}", exit_status));
        }
    }

    drop(tmp_swapfile);

    debug!("QEMU completed successfully");
    status_writer.finish()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_resolv_conf() {
        let cases = vec![
            // (input, expected)
            ("nameserver 8.8.8.8\n", vec!["8.8.8.8"]),
            (
                "nameserver 8.8.8.8\nnameserver 1.1.1.1\n",
                vec!["8.8.8.8", "1.1.1.1"],
            ),
            ("# comment\nnameserver 8.8.8.8\n", vec!["8.8.8.8"]),
            ("nameserver 127.0.0.1\n", vec!["127.0.0.1"]),
            ("nameserver 169.254.1.1\n", vec!["169.254.1.1"]),
            ("nameserver 10.0.0.1\n", vec!["10.0.0.1"]),
            (
                "nameserver 2001:4860:4860::8888\n",
                vec!["2001:4860:4860::8888"],
            ),
            ("nameserver ::1\n", vec!["::1"]),
            ("nameserver fe80::1\n", vec!["fe80::1"]),
            ("nameserver fc00::1\n", vec!["fc00::1"]),
            ("# only comments\n", vec![]),
            ("", vec![]),
        ];

        for (input, expected) in cases {
            assert_eq!(
                parse_resolv_conf(input),
                expected,
                "failed for input: {:?}",
                input
            );
        }
    }
}
