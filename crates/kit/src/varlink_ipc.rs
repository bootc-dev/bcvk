//! Varlink IPC interface for bcvk using the zlink crate.
//!
//! Exposes image listing, ephemeral VM launching, and disk image creation
//! over a Unix domain socket using the Varlink protocol. Three interfaces
//! are provided:
//!
//! - `io.bootc.vk.images` -- list bootc container image names
//! - `io.bootc.vk.ephemeral` -- list and launch ephemeral VM containers
//! - `io.bootc.vk.todisk` -- create bootable disk images from container images
//!
//! The API is intentionally minimal: it exposes only the operations that
//! require bcvk-specific knowledge (filtering by label, constructing the
//! right `podman run` invocation, orchestrating `bootc install`). Everything
//! else -- inspecting images, removing containers, executing commands -- is
//! left to `podman` directly.
//!
//! The server supports two activation modes:
//! - Direct listen: binds a Unix socket at the given `unix:` address
//! - Socket activation: when `LISTEN_FDS` is set (e.g. via `varlinkctl exec:`),
//!   serves on the inherited fd 3

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Reply wrapper types (varlink methods return named parameters)
// ---------------------------------------------------------------------------

/// Reply for the images `List` method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ListReply {
    /// Image names/tags that have `containers.bootc=1`.
    /// Each entry is a full image reference (e.g. `quay.io/centos-bootc/centos-bootc:stream9`).
    /// Dangling images (no tags) are omitted.
    images: Vec<String>,
}

/// Reply for the ephemeral `Ps` method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PsReply {
    /// Container IDs of running ephemeral VMs (label `bcvk.ephemeral=1`).
    /// Use `podman inspect` for further details.
    container_ids: Vec<String>,
}

/// Optional configuration for launching an ephemeral VM.
///
/// All fields are optional and default to sensible values.
/// The VM always runs detached (no TTY/interactive mode).
#[derive(Debug, Clone, Serialize, Deserialize, Default, zlink::introspect::Type)]
pub(crate) struct EphemeralRunOpts {
    /// Generate SSH keypair and inject into the VM.
    ssh_keygen: Option<bool>,
    /// Assign a name to the container.
    name: Option<String>,
    /// Metadata labels in `key=value` form.
    label: Option<Vec<String>>,
    /// Podman network configuration.
    network: Option<String>,
    /// Environment variables in `key=value` form.
    env: Option<Vec<String>>,
    /// Automatically remove container when it exits.
    rm: Option<bool>,
    /// Instance type (e.g. `"u1.small"`, `"u1.medium"`).
    itype: Option<String>,
    /// Memory size (e.g. `"4G"`, `"2048M"`); overridden by `itype`.
    memory: Option<String>,
    /// Number of vCPUs; overridden by `itype`.
    vcpus: Option<u32>,
    /// Connect QEMU console to container stdio (visible via `podman logs`/`attach`).
    console: Option<bool>,
    /// Read-write host bind mounts (`HOST_PATH[:NAME]`).
    bind: Option<Vec<String>>,
    /// Read-only host bind mounts (`HOST_PATH[:NAME]`).
    ro_bind: Option<Vec<String>>,
    /// Disk files as virtio-blk devices (`FILE[:NAME]`).
    mount_disk_files: Option<Vec<String>>,
    /// Additional kernel command line arguments.
    kargs: Option<Vec<String>>,
    /// Allocate swap of the given size (e.g. `"1G"`).
    add_swap: Option<String>,
}

/// Reply for the ephemeral `Run` method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RunReply {
    /// Container ID of the launched ephemeral VM.
    container_id: String,
}

/// Reply for the ephemeral `GetSshConnectionInfo` method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GetSshConnectionInfoReply {
    /// Container ID to use with `podman exec`.
    container_id: String,
    /// Path to the SSH private key *inside* the container.
    key_path: String,
    /// SSH user to connect as.
    user: String,
    /// SSH host (inside the container).
    host: String,
    /// SSH port.
    port: u16,
}

/// Reply for the `io.bootc.vk.todisk` `ToDisk` method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ToDiskReply {
    /// Absolute path to the created (or reused) disk image.
    path: String,
    /// Whether an existing cached disk image was reused.
    cached: bool,
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors returned by the images interface.
#[derive(Debug, zlink::ReplyError, zlink::introspect::ReplyError)]
#[zlink(interface = "io.bootc.vk.images")]
enum ImagesError {
    /// An error occurred when calling podman.
    PodmanError {
        /// Human-readable error description.
        message: String,
    },
}

/// Errors returned by the ephemeral interface.
#[derive(Debug, zlink::ReplyError, zlink::introspect::ReplyError)]
#[zlink(interface = "io.bootc.vk.ephemeral")]
enum EphemeralError {
    /// An error occurred when calling podman.
    PodmanError {
        /// Human-readable error description.
        message: String,
    },
}

/// Errors returned by the todisk interface.
#[derive(Debug, zlink::ReplyError, zlink::introspect::ReplyError)]
#[zlink(interface = "io.bootc.vk.todisk")]
enum ToDiskError {
    /// The disk image creation failed.
    Failed {
        /// Human-readable error description.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a `JoinError` (from `spawn_blocking`) to an `EphemeralError`.
fn ephemeral_join_err(e: tokio::task::JoinError) -> EphemeralError {
    EphemeralError::PodmanError {
        message: e.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Service implementation
// ---------------------------------------------------------------------------

/// Combined varlink service exposing images, ephemeral, and todisk interfaces.
#[derive(Debug)]
struct BcvkService;

/// Version of the varlink API itself (independent of the crate version).
/// Only referenced by the unit test that guards against attribute drift.
#[cfg(test)]
const VARLINK_API_VERSION: &str = "0.1.0";

#[zlink::service(
    interface = "io.bootc.vk.images",
    vendor = "io.bootc.vk",
    product = "bcvk",
    version = "0.1.0",
    url = "https://github.com/bootc-dev/bcvk"
)]
impl BcvkService {
    /// List bootc image names (those with `containers.bootc=1`).
    async fn list(&self) -> Result<ListReply, ImagesError> {
        let entries = tokio::task::spawn_blocking(crate::images::list)
            .await
            .map_err(|e| ImagesError::PodmanError {
                message: e.to_string(),
            })?
            .map_err(|e| ImagesError::PodmanError {
                message: e.to_string(),
            })?;

        let images = entries
            .into_iter()
            .filter_map(|img| img.names)
            .flatten()
            .collect();

        Ok(ListReply { images })
    }

    /// List ephemeral VM container IDs (those with `bcvk.ephemeral=1`).
    ///
    /// Returns only the container IDs; use `podman inspect <id>` for
    /// further details like state, image, or creation time.
    #[zlink(interface = "io.bootc.vk.ephemeral")]
    async fn ps(&self) -> Result<PsReply, EphemeralError> {
        let containers = tokio::task::spawn_blocking(crate::ephemeral::list_ephemeral_containers)
            .await
            .map_err(ephemeral_join_err)?
            .map_err(|e| EphemeralError::PodmanError {
                message: e.to_string(),
            })?;

        let container_ids = containers.into_iter().map(|c| c.id).collect();
        Ok(PsReply { container_ids })
    }

    /// Launch an ephemeral VM in detached mode.
    ///
    /// Always runs detached. The returned container ID can be used with
    /// `podman inspect`, `podman rm -f`, or passed to `GetSshConnectionInfo`.
    /// See [`EphemeralRunOpts`] for additional options.
    #[zlink(interface = "io.bootc.vk.ephemeral")]
    async fn run(
        &self,
        image: String,
        opts: Option<EphemeralRunOpts>,
    ) -> Result<RunReply, EphemeralError> {
        let opts = opts.unwrap_or_default();
        let container_id = tokio::task::spawn_blocking(move || {
            use crate::run_ephemeral::{CommonPodmanOptions, CommonVmOpts, RunEphemeralOpts};
            use std::str::FromStr;

            let itype = opts
                .itype
                .map(|s| {
                    crate::instancetypes::InstanceType::from_str(&s).map_err(|_| {
                        color_eyre::eyre::eyre!(
                            "unknown instance type: {s:?}, try e.g. \"u1.small\""
                        )
                    })
                })
                .transpose()?;

            let run_opts = RunEphemeralOpts {
                image,
                common: CommonVmOpts {
                    itype,
                    memory: crate::common_opts::MemoryOpts {
                        memory: opts
                            .memory
                            .unwrap_or_else(|| crate::common_opts::DEFAULT_MEMORY_USER_STR.into()),
                    },
                    vcpus: opts.vcpus,
                    console: opts.console.unwrap_or(false),
                    ssh_keygen: opts.ssh_keygen.unwrap_or(false),
                    ..Default::default()
                },
                podman: CommonPodmanOptions {
                    detach: true,
                    name: opts.name,
                    label: opts.label.unwrap_or_default(),
                    network: opts.network,
                    env: opts.env.unwrap_or_default(),
                    rm: opts.rm.unwrap_or(false),
                    ..Default::default()
                },
                debug_entrypoint: None,
                bind_mounts: opts.bind.unwrap_or_default(),
                ro_bind_mounts: opts.ro_bind.unwrap_or_default(),
                systemd_units_dir: None,
                bind_storage_ro: false,
                add_swap: opts.add_swap,
                mount_disk_files: opts.mount_disk_files.unwrap_or_default(),
                kernel_args: opts.kargs.unwrap_or_default(),
                host_dns_servers: None,
            };

            crate::run_ephemeral::run_detached(run_opts)
        })
        .await
        .map_err(ephemeral_join_err)?
        .map_err(|e| EphemeralError::PodmanError {
            message: e.to_string(),
        })?;

        Ok(RunReply { container_id })
    }

    /// Return the SSH connection details for an ephemeral VM.
    ///
    /// Given a container ID (from `Run` or `Ps`), returns the information
    /// needed to connect via SSH. The caller can then construct:
    ///
    /// ```text
    /// podman exec <container_id> ssh -i <key_path> -p <port> \
    ///     -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    ///     <user>@<host> [command...]
    /// ```
    ///
    /// This does not verify that SSH is actually ready -- the caller
    /// should retry on connection failure.
    #[zlink(interface = "io.bootc.vk.ephemeral")]
    async fn get_ssh_connection_info(
        &self,
        container_id: String,
    ) -> Result<GetSshConnectionInfoReply, EphemeralError> {
        // These are fixed conventions in bcvk's SSH setup.
        // The key path is inside the container (under the tmproot bind mount).
        let key_path = format!("/run/tmproot{}/ssh", crate::CONTAINER_STATEDIR);

        Ok(GetSshConnectionInfoReply {
            container_id,
            key_path,
            user: "root".to_string(),
            host: "127.0.0.1".to_string(),
            port: 2222,
        })
    }

    /// Create a bootable disk image from a container image.
    ///
    /// This is a long-running operation that orchestrates disk creation,
    /// ephemeral VM launch, and `bootc install`. Supports caching: if a
    /// disk already exists at `target_disk` with matching metadata, it is
    /// reused without reinstalling.
    ///
    /// Parameters:
    /// - `source_image`: container image reference to install
    /// - `target_disk`: absolute path for the output disk image
    /// - `format`: disk format, either `"raw"` (default) or `"qcow2"`
    /// - `disk_size`: optional size string (e.g. `"10G"`, `"5120M"`)
    /// - `filesystem`: optional root filesystem type (e.g. `"ext4"`, `"xfs"`)
    /// - `root_size`: optional root partition size (e.g. `"8G"`)
    /// - `kargs`: optional kernel arguments
    #[zlink(interface = "io.bootc.vk.todisk")]
    async fn to_disk(
        &self,
        source_image: String,
        target_disk: String,
        format: Option<String>,
        disk_size: Option<String>,
        filesystem: Option<String>,
        root_size: Option<String>,
        kargs: Option<Vec<String>>,
    ) -> Result<ToDiskReply, ToDiskError> {
        let result = tokio::task::spawn_blocking(move || {
            use camino::Utf8PathBuf;

            let format = match format.as_deref() {
                None | Some("raw") => crate::to_disk::Format::Raw,
                Some("qcow2") => crate::to_disk::Format::Qcow2,
                Some(other) => {
                    return Err(color_eyre::eyre::eyre!(
                        "unsupported disk format: {other:?}, expected \"raw\" or \"qcow2\""
                    ));
                }
            };

            let opts = crate::to_disk::ToDiskOpts {
                source_image,
                target_disk: Utf8PathBuf::from(&target_disk),
                install: crate::install_options::InstallOptions {
                    filesystem,
                    root_size,
                    karg: kargs.unwrap_or_default(),
                    ..Default::default()
                },
                additional: crate::to_disk::ToDiskAdditionalOpts {
                    disk_size,
                    format,
                    common: crate::run_ephemeral::CommonVmOpts {
                        memory: crate::common_opts::MemoryOpts {
                            memory: crate::common_opts::DEFAULT_MEMORY_USER_STR.into(),
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                },
            };

            let outcome = crate::to_disk::run(opts)?;
            let cached = outcome == crate::to_disk::RunOutcome::Cached;

            Ok::<_, color_eyre::Report>((target_disk, cached))
        })
        .await
        .map_err(|e| ToDiskError::Failed {
            message: e.to_string(),
        })?
        .map_err(|e| ToDiskError::Failed {
            message: format!("{e:#}"),
        })?;

        let (path, cached) = result;
        Ok(ToDiskReply { path, cached })
    }
}

// ---------------------------------------------------------------------------
// Client-side proxy traits (for future programmatic use)
// ---------------------------------------------------------------------------

/// Proxy for calling image management methods on a remote bcvk service.
#[allow(dead_code)]
#[zlink::proxy("io.bootc.vk.images")]
trait ImagesProxy {
    /// List bootc image names.
    async fn list(&mut self) -> zlink::Result<Result<ListReply, ImagesError>>;
}

/// Proxy for calling ephemeral container methods on a remote bcvk service.
#[allow(dead_code)]
#[zlink::proxy("io.bootc.vk.ephemeral")]
trait EphemeralProxy {
    /// List ephemeral VM container IDs.
    async fn ps(&mut self) -> zlink::Result<Result<PsReply, EphemeralError>>;

    /// Launch an ephemeral VM in detached mode.
    async fn run(
        &mut self,
        image: String,
        opts: Option<EphemeralRunOpts>,
    ) -> zlink::Result<Result<RunReply, EphemeralError>>;

    /// Get SSH connection info for an ephemeral VM.
    async fn get_ssh_connection_info(
        &mut self,
        container_id: String,
    ) -> zlink::Result<Result<GetSshConnectionInfoReply, EphemeralError>>;
}

/// Proxy for calling todisk methods on a remote bcvk service.
#[allow(dead_code)]
#[zlink::proxy("io.bootc.vk.todisk")]
trait ToDiskProxy {
    /// Create a bootable disk image from a container image.
    async fn to_disk(
        &mut self,
        source_image: String,
        target_disk: String,
        format: Option<String>,
        disk_size: Option<String>,
        filesystem: Option<String>,
        root_size: Option<String>,
        kargs: Option<Vec<String>>,
    ) -> zlink::Result<Result<ToDiskReply, ToDiskError>>;
}

// ---------------------------------------------------------------------------
// Socket activation
// ---------------------------------------------------------------------------

/// A `Listener` that yields a single pre-connected socket, then blocks forever.
///
/// Used for `varlinkctl exec:` activation where a connected socket pair is
/// passed on fd 3. After the first `accept()` returns the connection, subsequent
/// calls pend indefinitely (the server will be killed by the parent process once
/// the connection closes).
#[derive(Debug)]
struct ActivatedListener {
    /// The connection to yield on the first accept(), consumed after use.
    conn: Option<zlink::Connection<zlink::unix::Stream>>,
}

impl zlink::Listener for ActivatedListener {
    type Socket = zlink::unix::Stream;

    async fn accept(&mut self) -> zlink::Result<zlink::Connection<Self::Socket>> {
        match self.conn.take() {
            Some(conn) => Ok(conn),
            None => std::future::pending().await,
        }
    }
}

/// Try to build an [`ActivatedListener`] from a socket-activated fd.
///
/// Uses `libsystemd` to receive file descriptors passed by the service
/// manager (checks `LISTEN_FDS`/`LISTEN_PID` and clears the env vars).
/// Returns `None` when the process was not socket-activated.
#[allow(unsafe_code)]
fn try_activated_listener() -> color_eyre::Result<Option<ActivatedListener>> {
    use std::os::fd::{FromRawFd as _, IntoRawFd as _};

    let fds = libsystemd::activation::receive_descriptors(true)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to receive activation fds: {e}"))?;

    let fd = match fds.into_iter().next() {
        Some(fd) => fd,
        None => return Ok(None),
    };

    // TODO: propose From<FileDescriptor> for OwnedFd upstream so this
    // unsafe can be removed: https://github.com/lucab/libsystemd-rs
    //
    // SAFETY: `libsystemd::activation::receive_descriptors(true)` validated
    // the fd and transferred ownership. `into_raw_fd()` consumes the
    // `FileDescriptor` wrapper, giving us sole ownership of a valid fd.
    let std_stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd.into_raw_fd()) };
    std_stream.set_nonblocking(true)?;
    let tokio_stream = tokio::net::UnixStream::from_std(std_stream)?;
    let zlink_stream = zlink::unix::Stream::from(tokio_stream);
    let conn = zlink::Connection::from(zlink_stream);
    Ok(Some(ActivatedListener { conn: Some(conn) }))
}

// ---------------------------------------------------------------------------
// Varlink auto-activation
// ---------------------------------------------------------------------------

/// If the process was socket-activated, serve varlink and return `true`.
///
/// This follows the systemd pattern used by `systemd-creds` and similar
/// tools: if the process was invoked with an activated socket (e.g. via
/// `varlinkctl exec:`), it serves varlink on that socket and returns
/// `true` so the caller can exit. Otherwise returns `false` and the
/// process continues with normal CLI handling.
pub(crate) async fn try_serve_varlink() -> color_eyre::Result<bool> {
    let listener = match try_activated_listener()? {
        Some(l) => l,
        None => return Ok(false),
    };

    tracing::debug!("Socket activation detected, serving varlink");
    let server = zlink::Server::new(listener, BcvkService);
    tokio::select! {
        result = server.run() => result?,
        _ = tokio::signal::ctrl_c() => {
            tracing::debug!("Shutting down (activated)");
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::VARLINK_API_VERSION;

    #[test]
    fn varlink_version_is_consistent() {
        // The version in the #[zlink::service] attribute must match
        // VARLINK_API_VERSION. Unfortunately zlink doesn't let us use a
        // const in attribute position, so this test catches drift.
        assert_eq!(
            VARLINK_API_VERSION, "0.1.0",
            "VARLINK_API_VERSION must match the #[zlink::service] version attribute"
        );
    }
}
