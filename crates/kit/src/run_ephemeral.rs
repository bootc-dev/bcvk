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
use std::io::{BufWriter, IsTerminal, Seek, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use bootc_utils::CommandRunExt;
use camino::{Utf8Path, Utf8PathBuf};
use cap_std_ext::cmdext::CapStdExtCommandExt as _;
use clap::Parser;
use color_eyre::eyre::{eyre, Context};
use color_eyre::Result;
use rustix::fd::FromRawFd as _;
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

use crate::qemu::{self, QemuConfigExt};
use crate::{
    boot_progress,
    common_opts::MemoryOpts,
    podman,
    supervisor_status::{StatusWriter, SupervisorState, SupervisorStatus},
    systemd, utils, CONTAINER_STATEDIR,
};

/// fw_cfg name for Ignition configuration (per FCOS documentation)
const IGNITION_FW_CFG_NAME: &str = "opt/com.coreos/config";

/// virtio-blk serial name for Ignition configuration (per FCOS documentation)
const IGNITION_SERIAL_NAME: &str = "ignition";

/// Mount path for Ignition config inside the container
const IGNITION_CONFIG_MOUNT_PATH: &str = "/run/ignition-config.json";

// ---------------------------------------------------------------------------
// Journal / output mode types
// ---------------------------------------------------------------------------

/// Parsed value of `--log-dir=STREAMS=PATH`.
///
/// `STREAMS` is a comma-separated subset of `journal` and `console`; `PATH` is
/// a directory to write log files into.  Files written:
/// - `journal.json` when the `journal` stream is requested
/// - `console.txt`  when the `console` stream is requested
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogDir {
    pub path: std::path::PathBuf,
    pub journal: bool,
    pub console: bool,
}

impl LogDir {
    /// Returns the path for `journal.json` if the journal stream was requested.
    pub fn journal_path(&self) -> Option<std::path::PathBuf> {
        self.journal.then(|| self.path.join("journal.json"))
    }

    /// Returns the path for `journal-initrd.json` if the journal stream was requested.
    pub fn journal_initrd_path(&self) -> Option<std::path::PathBuf> {
        self.journal.then(|| self.path.join("journal-initrd.json"))
    }

    /// Returns the path for `console.txt` if the console stream was requested.
    pub fn console_path(&self) -> Option<std::path::PathBuf> {
        self.console.then(|| self.path.join("console.txt"))
    }
}

impl std::str::FromStr for LogDir {
    type Err = color_eyre::eyre::Error;

    fn from_str(s: &str) -> Result<Self> {
        // Split on the LAST `=` to support paths containing `=`.
        let eq = s.rfind('=').ok_or_else(|| {
            color_eyre::eyre::eyre!(
                "--log-dir value must have the form STREAMS=PATH (no `=` found): {s:?}"
            )
        })?;
        let streams_str = &s[..eq];
        let path_str = &s[eq + 1..];

        if path_str.is_empty() {
            return Err(color_eyre::eyre::eyre!(
                "--log-dir PATH must not be empty: {s:?}"
            ));
        }
        let path = std::path::PathBuf::from(path_str);

        let mut journal = false;
        let mut console = false;
        for part in streams_str.split(',') {
            match part.trim() {
                "journal" => journal = true,
                "console" => console = true,
                "" => {}
                other => {
                    return Err(color_eyre::eyre::eyre!(
                        "--log-dir unknown stream name {other:?}; expected `journal` or `console`"
                    ))
                }
            }
        }
        if !journal && !console {
            return Err(color_eyre::eyre::eyre!(
                "--log-dir STREAMS must contain at least one of `journal`, `console`; got: {streams_str:?}"
            ));
        }

        Ok(LogDir {
            path,
            journal,
            console,
        })
    }
}

#[cfg(test)]
mod logdir_tests {
    use super::*;
    use std::str::FromStr as _;

    #[test]
    fn test_log_dir_journal_only() {
        let ld = LogDir::from_str("journal=/tmp/logs").unwrap();
        assert!(ld.journal);
        assert!(!ld.console);
        assert_eq!(ld.path, std::path::PathBuf::from("/tmp/logs"));
        assert_eq!(
            ld.journal_path(),
            Some(std::path::PathBuf::from("/tmp/logs/journal.json"))
        );
        assert_eq!(ld.console_path(), None);
    }

    #[test]
    fn test_log_dir_console_only() {
        let ld = LogDir::from_str("console=/tmp/logs").unwrap();
        assert!(!ld.journal);
        assert!(ld.console);
        assert_eq!(
            ld.console_path(),
            Some(std::path::PathBuf::from("/tmp/logs/console.txt"))
        );
        assert_eq!(ld.journal_path(), None);
    }

    #[test]
    fn test_log_dir_both() {
        let ld = LogDir::from_str("journal,console=/tmp/run-001/").unwrap();
        assert!(ld.journal);
        assert!(ld.console);
        assert_eq!(ld.path, std::path::PathBuf::from("/tmp/run-001/"));
    }

    #[test]
    fn test_log_dir_path_no_trailing_slash() {
        let ld = LogDir::from_str("journal=/var/tmp/run-001").unwrap();
        assert_eq!(ld.path, std::path::PathBuf::from("/var/tmp/run-001"));
        assert_eq!(
            ld.journal_path(),
            Some(std::path::PathBuf::from("/var/tmp/run-001/journal.json"))
        );
    }

    #[test]
    fn test_log_dir_error_no_equals() {
        assert!(LogDir::from_str("journal/tmp/logs").is_err());
    }

    #[test]
    fn test_log_dir_error_empty_path() {
        assert!(LogDir::from_str("journal=").is_err());
    }

    #[test]
    fn test_log_dir_error_unknown_stream() {
        assert!(LogDir::from_str("foo=/tmp/logs").is_err());
    }

    #[test]
    fn test_log_dir_error_no_streams() {
        // Empty streams string before `=`
        assert!(LogDir::from_str("=/tmp/logs").is_err());
    }
}

/// Controls where the VM's systemd journal is streamed.
///
/// - `Console` (default): no journal capture; the VM's hvc0 console is connected
///   to the container's stdio as usual.
/// - `Journal`: stream the journal as plain text to stdout.
///
/// To capture the journal as JSON to a file, use `--log-dir=journal=PATH`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum OutputMode {
    #[default]
    Console,
    Journal,
}

/// The guest-side systemd unit that streams the journal as JSON over virtio-serial.
/// Always uses JSON format; the host converts to plain text for stdout as needed.
pub(crate) const JOURNAL_STREAM_UNIT: &str = include_str!("units/bcvk-journal-stream.service");
pub(crate) const JOURNAL_STREAM_INITRD_UNIT: &str =
    include_str!("units/bcvk-journal-stream-initrd.service");

/// Convert a single JSON journal line (as produced by `journalctl -o json`) into
/// a human-readable text line suitable for printing to stdout.
///
/// Returns `None` if the JSON has no `MESSAGE` field or if parsing fails.
///
/// The prefix mirrors systemd's `with-unit` output mode (`journalctl -o with-unit`):
/// - Unit: `_SYSTEMD_UNIT[/_SYSTEMD_USER_UNIT]`, falling back to
///   `SYSLOG_IDENTIFIER` → `_COMM` → `"unknown"`
/// - PID: `[_PID]` or `[SYSLOG_PID]` if present
/// - Then `: MESSAGE`
pub(crate) fn journal_json_to_text(line: &str) -> Option<String> {
    let obj = serde_json::from_str::<serde_json::Value>(line).ok()?;
    let message = obj.get("MESSAGE").and_then(|v| v.as_str())?;

    let str_field = |key: &str| obj.get(key).and_then(|v| v.as_str());

    // Build the unit/identifier prefix, matching systemd's OUTPUT_WITH_UNIT logic.
    let unit = str_field("_SYSTEMD_UNIT");
    let user_unit = str_field("_SYSTEMD_USER_UNIT");
    let prefix = if unit.is_some() || user_unit.is_some() {
        match (unit, user_unit) {
            (Some(u), Some(uu)) => format!("{u}/{uu}"),
            (Some(u), None) => u.to_owned(),
            (None, Some(uu)) => uu.to_owned(),
            (None, None) => unreachable!(),
        }
    } else if let Some(id) = str_field("SYSLOG_IDENTIFIER") {
        id.to_owned()
    } else if let Some(comm) = str_field("_COMM") {
        comm.to_owned()
    } else {
        "unknown".to_owned()
    };

    // Append [PID] when available, preferring the trusted _PID field.
    let pid_suffix = str_field("_PID")
        .or_else(|| str_field("SYSLOG_PID"))
        .map(|p| format!("[{p}]"))
        .unwrap_or_default();

    Some(format!("{prefix}{pid_suffix}: {message}\n"))
}

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

    #[clap(
        long,
        help = "Connect the QEMU console to the container's stdio (visible via podman logs/attach)"
    )]
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

    #[clap(
        long = "virtiofsd",
        env = "VIRTIOFSD_BIN",
        help = "Path to virtiofsd binary (overrides auto-detection)"
    )]
    pub virtiofsd_binary: Option<String>,

    /// Select how VM output is presented.
    ///
    /// `console` (default): the VM's hvc0 console is forwarded to stdio.
    /// `journal`: the systemd journal is streamed as plain text to stdout.
    ///
    /// To capture streams to files, use `--log-dir`.
    #[clap(long, value_enum, default_value = "console")]
    pub output: OutputMode,

    /// Write VM log streams to files in DIR.
    ///
    /// STREAMS is a comma-separated list of: `journal`, `console`
    /// - `journal` → `journal.json` (systemd journal as JSON)
    /// - `console` → `console.txt` (VM serial console output)
    ///
    /// Examples: `--log-dir=journal,console=/tmp/run-001/`
    ///           `--log-dir=journal=/tmp/logs/`
    #[clap(long, value_name = "STREAMS=DIR")]
    pub log_dir: Option<LogDir>,
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

    #[clap(
        long = "ignition",
        help = "Path to Ignition config file (JSON format) to inject via fw_cfg"
    )]
    pub ignition_config: Option<String>,

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
    let (mut cmd, temp_dir, _journal_fds) = prepare_run_command_with_temp(opts)?;

    // Leak the tempdir to keep it alive for the entire container lifetime.
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
    // Print helpful hint when running in foreground mode without console
    if !opts.podman.detach && !opts.common.console && std::io::stderr().is_terminal() {
        if let Some(name) = &opts.podman.name {
            eprintln!(
                "Hint: Use 'bcvk ephemeral ssh {}' to connect, or add --console to see VM output",
                name
            );
        } else {
            eprintln!("Hint: Add --console to see VM output, or use -d for background mode");
        }
    }

    let (mut cmd, _temp_dir, _journal_fds) = prepare_run_command_with_temp(opts)?;

    // Keep _temp_dir and _journal_fds alive until exec replaces our process.
    // The journal fds (if any) are inherited across execve and reach podman
    // via --preserve-fd; podman in turn passes them into the container.
    return Err(cmd.exec()).context("execve");
}

/// Returns `(cmd, tempdir, journal_fds)` where `journal_fds` holds open file
/// descriptors for `journal.json` and `journal-initrd.json` (when
/// `--log-dir=journal=…` was requested).  The caller must keep them alive until
/// podman exits so the fds are not closed prematurely.
fn prepare_run_command_with_temp(
    opts: RunEphemeralOpts,
) -> Result<(
    std::process::Command,
    tempfile::TempDir,
    Vec<std::sync::Arc<rustix::fd::OwnedFd>>,
)> {
    debug!("Running QEMU inside hybrid container for {}", opts.image);

    // Check Ignition support early (before launching container) if --ignition is specified
    if opts.ignition_config.is_some() {
        let has_ignition = check_ignition_support(&opts.image)?;
        if !has_ignition {
            return Err(eyre!(
                "Image does not support Ignition. See man bcvk-ephemeral-run for details."
            ));
        }
        debug!("Image {} supports Ignition", opts.image);
    }

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
        // Devices like /dev/kvm with group access need those groups propagated to the
        // container so commands in the container can use them
        "--group-add=keep-groups",
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
        // Some host distributions don't have all libraries in /lib or /lib64.
        // We mount the current ld.so.cache into the tmproot so it matches
        // library locations in the mounted /usr
        "/etc/ld.so.cache:/run/tmproot/etc/ld.so.cache:ro",
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

    // Mount Ignition config file if specified
    if let Some(ref ignition_path) = opts.ignition_config {
        // Convert to absolute path if needed
        let path = Utf8Path::new(ignition_path);
        let ignition_abs = if path.is_absolute() {
            path.to_owned()
        } else {
            let current_dir = Utf8PathBuf::try_from(std::env::current_dir()?)
                .context("Current directory path is not valid UTF-8")?;
            current_dir.join(path)
        };

        // Just validate we can access the file here, we pass the path
        // to podman as a bind mount which will reopen.
        if !ignition_abs.try_exists()? {
            return Err(eyre!("Ignition config file not found: {}", ignition_abs));
        }

        cmd.args([
            "-v",
            &format!("{}:{}:ro", ignition_abs, IGNITION_CONFIG_MOUNT_PATH),
        ]);
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

    // Open journal log files (from --log-dir) on the host and pass their fds into
    // the container via --preserve-fd / BCVK_JOURNAL_FDS=fd1,fd2 where fd1 is
    // journal.json (real-root) and fd2 is journal-initrd.json (initrd).
    // We use cap-std-ext's CmdFds / take_fds to handle fd duplication and
    // O_CLOEXEC clearing in a pre_exec hook; flatpak-spawn --forward-fd (in our
    // podman wrapper) ensures the fds also reach the real host podman from inside
    // a toolbox.
    let mut journal_fds: Vec<std::sync::Arc<rustix::fd::OwnedFd>> = Vec::new();
    if opts.common.log_dir.as_ref().map_or(false, |d| d.journal) {
        let mut fds = cap_std_ext::cmdext::CmdFds::new();
        let mut fd_nums = Vec::new();
        for dest in [
            opts.common.log_dir.as_ref().and_then(|d| d.journal_path()),
            opts.common
                .log_dir
                .as_ref()
                .and_then(|d| d.journal_initrd_path()),
        ]
        .into_iter()
        .flatten()
        {
            use std::os::unix::io::IntoRawFd as _;
            let f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&dest)
                .with_context(|| format!("Opening journal log file {dest:?}"))?;
            // SAFETY: we just opened this file and are transferring ownership into OwnedFd.
            #[allow(unsafe_code)]
            let owned = unsafe { rustix::fd::OwnedFd::from_raw_fd(f.into_raw_fd()) };
            let owned = std::sync::Arc::new(owned);
            let fd_n = fds.take_fd(owned.clone());
            cmd.args(["--preserve-fd", &fd_n.to_string()]);
            fd_nums.push(fd_n.to_string());
            journal_fds.push(owned);
        }
        cmd.take_fds(fds);
        cmd.args(["-e", &format!("BCVK_JOURNAL_FDS={}", fd_nums.join(","))]);
    }

    // If a console log path was requested via --log-dir, bind-mount its parent
    // directory into the container and tell run_impl the in-container path via
    // BCVK_CONSOLE_PATH.  QEMU writes the serial console output there directly
    // (no fd-passing needed — QEMU uses `-serial file:<path>`).
    if let Some(console_path) = opts.common.log_dir.as_ref().and_then(|d| d.console_path()) {
        // Resolve the parent to an absolute path so the bind mount is unambiguous.
        let parent = console_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let abs_parent = parent
            .canonicalize()
            .with_context(|| format!("--log-dir console parent directory not found: {parent:?}"))?;
        let file_name = console_path
            .file_name()
            .ok_or_else(|| eyre!("--log-dir console path has no file name: {console_path:?}"))?;
        // Fixed in-container mount point for the console log's parent directory.
        let container_log_dir = "/run/bcvk-console-log";
        cmd.args([
            "-v",
            &format!("{}:{}:rw", abs_parent.display(), container_log_dir),
        ]);
        let in_container_path = format!("{}/{}", container_log_dir, file_name.to_string_lossy());
        cmd.args(["-e", &format!("BCVK_CONSOLE_PATH={in_container_path}")]);
        debug!("Bind-mounting console log dir {abs_parent:?} → {container_log_dir}");
    }

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
    cmd.args(["--", &opts.image, entrypoint]);

    Ok((cmd, td, journal_fds))
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
/// Auto-enables .mount units in remote-fs.target.wants/, preserves default.target.wants/ symlinks.
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
    fs::create_dir_all(&format!("{}/remote-fs.target.wants", target_units))?;

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
                let wants_dir = format!("{}/remote-fs.target.wants", target_units);
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

/// These binaries must be present in the privileged container that runs bcvk,
/// not the guest bootc image that gets booted inside the VM.
fn check_required_container_binaries() -> Result<()> {
    // systemctl: used for checking cloud-init and other systemd operations
    // objcopy: for UKI kernel extraction (when using UKI images)
    // NOTE: bwrap is checked earlier in entrypoint.sh, not here, because by the
    // time run_impl() executes we're already inside the bwrap namespace
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

/// Check if the container image has Ignition support
///
/// Checks for labels indicating Ignition support:
/// - 'coreos.ignition' (future convention, not yet widely used)
/// - 'com.coreos.osname' (heuristic: CoreOS-based images likely have Ignition)
///
/// Returns true if the image is likely to support Ignition.
fn check_ignition_support(image: &str) -> Result<bool> {
    use std::collections::HashMap;
    use std::process::Stdio;

    // Fetch all labels with a single podman inspect call
    let output = Command::new("podman")
        .args(["image", "inspect", "--format", "{{json .Labels}}", image])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to inspect image for labels")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!(
            "Failed to inspect image {} for labels: {}",
            image,
            stderr.trim()
        ));
    }

    // Parse the JSON output
    let labels: HashMap<String, String> =
        serde_json::from_slice(&output.stdout).context("Failed to parse image labels as JSON")?;

    // Check for coreos.ignition label (could contain version info or just "1")
    if let Some(ignition_value) = labels.get("coreos.ignition") {
        if !ignition_value.is_empty() {
            debug!(
                "Image {} has coreos.ignition={} label",
                image, ignition_value
            );
            return Ok(true);
        }
    }

    // Fallback: check for com.coreos.osname (CoreOS-based images)
    if let Some(osname_value) = labels.get("com.coreos.osname").filter(|v| !v.is_empty()) {
        debug!(
            "Image {} has com.coreos.osname={}, assuming Ignition support",
            image, osname_value
        );
        return Ok(true);
    }

    debug!("Image {} does not appear to support Ignition", image);
    Ok(false)
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

    // Find kernel and initramfs using the kernel detection module
    let source_root = cap_std_ext::cap_std::fs::Dir::open_ambient_dir(
        "/run/source-image",
        cap_std_ext::cap_std::ambient_authority(),
    )
    .context("opening /run/source-image")?;

    let kernel_info = crate::kernel::find_kernel(&source_root)
        .context("searching for kernel")?
        .ok_or_else(|| {
            eyre!(
                "No kernel found. Checked:\n\
                 - /boot/EFI/Linux/*.efi (UKI)\n\
                 - /usr/lib/modules/<version>/<version>.efi (UKI)\n\
                 - /usr/lib/modules/<version>/vmlinuz + initramfs.img"
            )
        })?;

    // Add the source-image prefix to get absolute paths
    let kernel_info =
        crate::kernel::with_root_prefix(kernel_info, Utf8Path::new("/run/source-image"));

    debug!(
        "Found kernel: {:?} (UKI: {})",
        kernel_info.kernel_path, kernel_info.is_uki
    );

    let kernel_mount = "/run/qemu/kernel";
    let initramfs_mount = "/run/qemu/initramfs";

    // Extract from UKI if found, otherwise use traditional kernel
    if kernel_info.is_uki {
        debug!(
            "Extracting kernel and initramfs from UKI: {:?}",
            kernel_info.kernel_path
        );

        // Extract .linux section (kernel) from UKI
        Command::new("objcopy")
            .args([
                "--dump-section",
                &format!(".linux={}", kernel_mount),
                kernel_info.kernel_path.as_str(),
            ])
            .run_capture_stderr()
            .map_err(|e| eyre!("Failed to extract kernel from UKI: {e}"))?;
        debug!("Extracted kernel from UKI to {}", kernel_mount);

        // Extract .initrd section (initramfs) from UKI
        Command::new("objcopy")
            .args([
                "--dump-section",
                &format!(".initrd={}", initramfs_mount),
                kernel_info.kernel_path.as_str(),
            ])
            .run_capture_stderr()
            .map_err(|e| eyre!("Failed to extract initramfs from UKI: {e}"))?;
        debug!("Extracted initramfs from UKI to {}", initramfs_mount);
    } else {
        let source_initramfs_path = kernel_info
            .initramfs_path
            .as_ref()
            .ok_or_else(|| eyre!("Traditional kernel found but no initramfs path"))?;

        fs::File::create(kernel_mount)?;

        // Bind mount kernel (read-only is fine)
        Command::new("mount")
            .args([
                "--bind",
                "-o",
                "ro",
                kernel_info.kernel_path.as_str(),
                kernel_mount,
            ])
            .run_capture_stderr()
            .map_err(|e| eyre!("Failed to bind mount kernel: {e}"))?;

        // Copy initramfs so we can append to it
        fs::copy(source_initramfs_path, initramfs_mount)
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
                virtiofsd_binary: opts.common.virtiofsd_binary.as_deref().map(Into::into),
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

            // Collect unit name for the remote-fs.target dropin
            mount_unit_names.push(unit_name.clone());

            debug!(
                "Generated SMBIOS credential for mount unit: {} ({})",
                unit_name, mode
            );
        }
    }

    // If we have mount units, create a single dropin for remote-fs.target.
    // We use remote-fs.target because virtiofs is conceptually similar to a remote
    // filesystem - it requires virtio transport infrastructure, like NFS needs network.
    if !mount_unit_names.is_empty() {
        let wants_list = mount_unit_names.join(" ");
        let dropin_content = format!("[Unit]\nWants={}\n", wants_list);
        let encoded_dropin = data_encoding::BASE64.encode(dropin_content.as_bytes());
        let dropin_cred = format!(
            "io.systemd.credential.binary:systemd.unit-dropin.remote-fs.target~bcvk-mounts={encoded_dropin}"
        );
        mount_unit_smbios_creds.push(dropin_cred);
        debug!(
            "Created remote-fs.target dropin for {} mount units",
            mount_unit_names.len()
        );
    }

    // Note: /etc overlay and /var ephemeral units are now embedded directly in the
    // initramfs CPIO (see cpio.rs) rather than injected via SMBIOS credentials.
    // This ensures they work on systemd <256 where credential import happens too
    // late for generators to process.

    // Handle --execute: pipes will be created when adding to qemu_config later
    // No need to create files anymore as we're using pipes

    // Inject the journal streaming unit when either --output=journal or
    // --log-dir with journal stream is requested.  The guest always streams JSON;
    // the host converts JSON→plain-text for stdout as needed.
    let wants_journal_stream = opts.common.output == OutputMode::Journal
        || opts.common.log_dir.as_ref().map_or(false, |d| d.journal);
    if wants_journal_stream {
        let encoded_journal = data_encoding::BASE64.encode(JOURNAL_STREAM_UNIT.as_bytes());
        mount_unit_smbios_creds.push(format!(
            "io.systemd.credential.binary:systemd.extra-unit.bcvk-journal-stream.service={encoded_journal}"
        ));
        let encoded_journal_initrd =
            data_encoding::BASE64.encode(JOURNAL_STREAM_INITRD_UNIT.as_bytes());
        mount_unit_smbios_creds.push(format!(
            "io.systemd.credential.binary:systemd.extra-unit.bcvk-journal-stream-initrd.service={encoded_journal_initrd}"
        ));
        debug!("Injected SMBIOS credentials for journal streaming units (real-root + initrd)");

        let journal_dropin =
            "[Unit]\nWants=bcvk-journal-stream.service bcvk-journal-stream-initrd.service\n";
        let encoded_dropin = data_encoding::BASE64.encode(journal_dropin.as_bytes());
        mount_unit_smbios_creds.push(format!(
            "io.systemd.credential.binary:systemd.unit-dropin.sysinit.target~bcvk-journal={encoded_dropin}"
        ));
        debug!("Created sysinit.target dropin to enable journal streaming units");
    }

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
    main_virtiofsd_config.virtiofsd_binary =
        opts.common.virtiofsd_binary.as_deref().map(Into::into);

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

    // Build kernel command line for direct boot.
    //
    // We deliberately omit root=, rootfstype=, and rootflags= from the
    // cmdline.  When root= is absent dracut sets rootok=1 via its UNSET
    // branch and defers entirely to systemd generators.  systemd-fstab-
    // generator likewise produces nothing without a root= arg.  The
    // virtiofs mount is handled solely by the sysroot.mount unit bcvk
    // injects into every initramfs via the CPIO append, together with the
    // initrd-root-fs.target.d/bcvk-sysroot.conf drop-in that wires it in.
    let mut kernel_cmdline = [
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

    // Add Ignition platform kernel argument if Ignition config is specified
    // This tells Ignition which platform it's running on and where to find the config
    if opts.ignition_config.is_some() {
        kernel_cmdline.push("ignition.platform.id=qemu".to_string());
    }

    kernel_cmdline.extend(opts.kernel_args.clone());
    qemu_config.set_kernel_cmdline(kernel_cmdline);

    // Add Ignition config if specified
    // Different architectures require different methods (per FCOS docs):
    // - x86_64/aarch64: fw_cfg
    // - s390x/ppc64le: virtio-blk with serial "ignition"
    if opts.ignition_config.is_some() {
        let ignition_path = Utf8Path::new(IGNITION_CONFIG_MOUNT_PATH);
        if !ignition_path.exists() {
            return Err(eyre!(
                "Ignition config not found at expected location: {}\n\
                 This is an internal error - the config should have been mounted by podman.",
                ignition_path
            ));
        }

        let arch = std::env::consts::ARCH;
        match arch {
            "x86_64" | "aarch64" => {
                debug!("Adding Ignition config via fw_cfg: {}", ignition_path);
                qemu_config.add_fw_cfg(IGNITION_FW_CFG_NAME.to_string(), ignition_path.to_owned());
            }
            "s390x" | "powerpc64le" => {
                debug!("Adding Ignition config via virtio-blk: {}", ignition_path);
                qemu_config.add_virtio_blk_device_with_format_ro(
                    ignition_path.to_string(),
                    IGNITION_SERIAL_NAME.to_string(),
                    crate::to_disk::Format::Raw,
                    true, // readonly as required by FCOS
                );
            }
            _ => {
                return Err(eyre!(
                    "Ignition config injection not supported on architecture: {}\n\
                     Supported architectures: x86_64, aarch64, s390x, powerpc64le",
                    arch
                ));
            }
        }
    }

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
            .run_capture_stderr()
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
                    format: format.into(),
                    readonly: false,
                });
            }
        }
    }

    qemu_config.set_console(opts.common.console);

    // Set serial console log path if provided via --log-dir=console=...
    // The host bound-mounted the parent directory; BCVK_CONSOLE_PATH is the
    // in-container path that QEMU can write to directly.
    if let Ok(console_path) = std::env::var("BCVK_CONSOLE_PATH") {
        if !console_path.is_empty() {
            qemu_config.serial_log = Some(console_path);
        }
    }

    // Set up virtio-serial pipes for journal streaming when requested.
    // add_virtio_serial_pipe() creates a pipe, passes the write end to QEMU via
    // --add-fd/fdset, and returns the read end.  We spawn async tasks to drain
    // them; they are awaited after QEMU exits to flush all data.
    //
    // Two ports are used:
    //   org.bcvk.journal        → journal.json
    //   org.bcvk.journal.initrd → journal-initrd.json
    //
    // The host files were opened before execve and their fds passed in via
    // BCVK_JOURNAL_FDS=fd1,fd2 (fd1=journal.json, fd2=journal-initrd.json).
    let mut worker_tasks = tokio::task::JoinSet::new();
    if wants_journal_stream {
        let read_file: std::fs::File = qemu_config
            .add_virtio_serial_pipe("org.bcvk.journal")?
            .into();
        let initrd_read_file: std::fs::File = qemu_config
            .add_virtio_serial_pipe("org.bcvk.journal.initrd")?
            .into();
        debug!("Added virtio-serial pipes for journal streaming (real-root + initrd)");

        let stdout_wants_journal = opts.common.output == OutputMode::Journal;

        // Parse BCVK_JOURNAL_FDS=fd1,fd2 to reconstruct the two host file writers.
        let wants_journal_file = opts.common.log_dir.as_ref().map_or(false, |d| d.journal);
        let (file_writer, initrd_file_writer): (Option<tokio::fs::File>, Option<tokio::fs::File>) =
            if wants_journal_file {
                use std::os::unix::io::FromRawFd as _;
                let fds_str = std::env::var("BCVK_JOURNAL_FDS")
                    .context("BCVK_JOURNAL_FDS not set but --log-dir=journal=... was given")?;
                let mut parts = fds_str.splitn(2, ',');
                let fd1: i32 = parts
                    .next()
                    .unwrap_or("")
                    .parse()
                    .context("BCVK_JOURNAL_FDS: invalid first fd")?;
                let fd2: i32 = parts
                    .next()
                    .unwrap_or("")
                    .parse()
                    .context("BCVK_JOURNAL_FDS: invalid second fd")?;
                // SAFETY: fds were opened by the host process and preserved through
                // execve / flatpak-spawn --forward-fd / podman --preserve-fd.
                // We are the sole owner at this point.
                #[allow(unsafe_code)]
                let (f1, f2) = unsafe {
                    (
                        std::fs::File::from_raw_fd(fd1),
                        std::fs::File::from_raw_fd(fd2),
                    )
                };
                (
                    Some(tokio::fs::File::from_std(f1)),
                    Some(tokio::fs::File::from_std(f2)),
                )
            } else {
                (None, None)
            };

        // Spawn the real-root journal drain task.
        let reader = tokio::fs::File::from_std(read_file);
        worker_tasks.spawn(async move {
            use tokio::io::AsyncBufReadExt as _;
            use tokio::io::AsyncWriteExt as _;
            let mut file_writer = file_writer;
            let mut stdout = tokio::io::stdout();
            let mut lines = tokio::io::BufReader::new(reader).lines();
            loop {
                match lines.next_line().await {
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("journal pipe read error: {e}");
                        break;
                    }
                    Ok(Some(line)) => {
                        if let Some(ref mut fw) = file_writer {
                            if let Err(e) = fw.write_all(format!("{line}\n").as_bytes()).await {
                                tracing::warn!("Failed to write journal JSON to file: {e}");
                                file_writer = None;
                            }
                        }
                        if stdout_wants_journal {
                            if let Some(text) = journal_json_to_text(&line) {
                                if let Err(e) = stdout.write_all(text.as_bytes()).await {
                                    tracing::warn!("Failed to write journal text to stdout: {e}");
                                }
                            }
                        }
                    }
                }
            }
            tracing::debug!("journal copy task done");
        });

        // Spawn the initrd journal drain task (writes to journal-initrd.json only).
        let initrd_reader = tokio::fs::File::from_std(initrd_read_file);
        worker_tasks.spawn(async move {
            use tokio::io::AsyncBufReadExt as _;
            use tokio::io::AsyncWriteExt as _;
            let mut file_writer = initrd_file_writer;
            let mut lines = tokio::io::BufReader::new(initrd_reader).lines();
            loop {
                match lines.next_line().await {
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("initrd journal pipe read error: {e}");
                        break;
                    }
                    Ok(Some(line)) => {
                        if let Some(ref mut fw) = file_writer {
                            if let Err(e) = fw.write_all(format!("{line}\n").as_bytes()).await {
                                tracing::warn!("Failed to write initrd journal JSON to file: {e}");
                                file_writer = None;
                            }
                        }
                    }
                }
            }
            tracing::debug!("initrd journal copy task done");
        });
    }

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

        // Drain any remaining journal output (pipe write end closed when QEMU exits)
        worker_tasks.join_all().await;

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
        // Drain any remaining journal output (pipe write end closed when QEMU exits)
        worker_tasks.join_all().await;
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
    fn test_journal_json_to_text() {
        // _SYSTEMD_UNIT takes priority over SYSLOG_IDENTIFIER, with PID
        let line = r#"{"MESSAGE":"started","_SYSTEMD_UNIT":"foo.service","SYSLOG_IDENTIFIER":"foo","_PID":"42"}"#;
        assert_eq!(
            journal_json_to_text(line),
            Some("foo.service[42]: started\n".into())
        );

        // _SYSTEMD_UNIT + _SYSTEMD_USER_UNIT joined with /
        let line =
            r#"{"MESSAGE":"hi","_SYSTEMD_UNIT":"app.service","_SYSTEMD_USER_UNIT":"user.service"}"#;
        assert_eq!(
            journal_json_to_text(line),
            Some("app.service/user.service: hi\n".into())
        );

        // Falls back to SYSLOG_IDENTIFIER when no unit fields; SYSLOG_PID used
        let line = r#"{"MESSAGE":"hello","SYSLOG_IDENTIFIER":"myapp","SYSLOG_PID":"99"}"#;
        assert_eq!(
            journal_json_to_text(line),
            Some("myapp[99]: hello\n".into())
        );

        // Falls back to _COMM when no unit or identifier
        let line = r#"{"MESSAGE":"from comm","_COMM":"bash"}"#;
        assert_eq!(journal_json_to_text(line), Some("bash: from comm\n".into()));

        // Falls back to "unknown" with no identifying fields
        let line = r#"{"MESSAGE":"bare message"}"#;
        assert_eq!(
            journal_json_to_text(line),
            Some("unknown: bare message\n".into())
        );

        // No MESSAGE → None
        let line = r#"{"SYSLOG_IDENTIFIER":"foo"}"#;
        assert_eq!(journal_json_to_text(line), None);

        // Invalid JSON → None
        assert_eq!(journal_json_to_text("not json at all"), None);
    }

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
