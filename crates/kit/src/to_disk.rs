//! Install bootc images to disk using ephemeral VMs
//!
//! This module provides the core installation functionality for bcvk, enabling
//! automated installation of bootc container images to disk images through an
//! ephemeral VM-based approach.
//!
//! # Installation Workflow
//!
//! The bootc installation process follows these key steps:
//!
//! 1. **Image Preparation**: Validates the source container image and prepares the
//!    target disk file, creating it with appropriate sizing if it doesn't exist
//!
//! 2. **Storage Configuration**: Sets up container storage access within the
//!    installation VM by mounting the host's container storage as read-only
//!
//! 3. **Ephemeral VM Launch**: Creates a temporary VM using the bootc image itself
//!    as the installation environment, with the target disk attached via virtio-blk
//!
//! 4. **Bootc Installation**: Executes `bootc install to-disk` within the VM,
//!    installing the container image to the attached disk with the specified
//!    filesystem and configuration options
//!
//! 5. **Cleanup**: The ephemeral VM automatically shuts down after installation,
//!    leaving behind the configured disk image ready for deployment
//!
//! # Disk Image Management
//!
//! The installation process creates and manages disk images as follows:
//!
//! - **Automatic Sizing**: Target disk size is calculated as 2x the source image
//!   size with a 4GB minimum to ensure adequate space for installation
//!
//! - **File Creation**: Creates sparse disk image files that grow as needed,
//!   supporting efficient storage usage
//!
//! - **Virtio-blk Attachment**: Attaches the target disk to the VM using virtio-blk
//!   with a predictable device name (`/dev/disk/by-id/virtio-output`)
//!
//! # Filesystem and Storage Options
//!
//! The module supports multiple filesystem types and storage configurations:
//!
//! - **Filesystem Types**: ext4 (default), xfs, and btrfs filesystems
//! - **Custom Root Size**: Optional specification of root filesystem size
//! - **Storage Path Detection**: Automatic detection of host container storage or
//!   manual specification for custom setups
//!
//! # Ephemeral VM Integration
//!
//! This module leverages the ephemeral VM infrastructure (`run_ephemeral`) to:
//!
//! - **Isolated Environment**: Provides a clean, isolated environment for
//!   installation without affecting the host system
//!
//! - **Container Storage Access**: Mounts host container storage read-only to
//!   access the source image without network dependencies
//!
//! - **Automated Lifecycle**: Handles VM startup, installation execution, and
//!   cleanup automatically with proper error handling
//!
//! - **Debug Support**: Provides comprehensive logging and debug output for
//!   troubleshooting installation issues
//!
//! # Usage Examples
//!
//! ```bash
//! # Basic installation with defaults
//! bcvk to-disk quay.io/centos-bootc/centos-bootc:stream10 output.img
//!
//! # Custom filesystem and size
//! bcvk to-disk --filesystem xfs --root-size 20G \
//!     quay.io/centos-bootc/centos-bootc:stream10 output.img
//! ```

use std::io::IsTerminal;

use crate::cache_metadata::DiskImageMetadata;
use crate::install_options::InstallOptions;
use crate::run_ephemeral::{run_detached, CommonVmOpts, RunEphemeralOpts};
use crate::run_ephemeral_ssh::wait_for_ssh_ready;
use crate::{images, ssh, utils};
use camino::{Utf8Path, Utf8PathBuf};
use clap::{Parser, ValueEnum};
use color_eyre::eyre::{eyre, Context};
use color_eyre::Result;
use indicatif::HumanDuration;
use indoc::indoc;
use tempfile::TempDir;
use tracing::debug;

/// Supported disk image formats
#[derive(Debug, Clone, ValueEnum, PartialEq, Default)]
pub enum Format {
    /// Raw disk image format (default)
    #[default]
    Raw,
    /// QEMU Copy On Write 2 format
    Qcow2,
}

impl Format {
    /// Get the string representation for qemu-img
    pub fn as_str(&self) -> &'static str {
        match self {
            Format::Raw => "raw",
            Format::Qcow2 => "qcow2",
        }
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Additional configuration options for installing a bootc container image to disk
#[derive(Debug, Parser, Default)]
pub struct ToDiskAdditionalOpts {
    /// Disk size to create (e.g. 10G, 5120M, or plain number for bytes)
    #[clap(long)]
    pub disk_size: Option<String>,

    /// Output disk image format
    #[clap(long, default_value_t = Format::Raw)]
    pub format: Format,

    /// Common VM configuration options
    #[clap(flatten)]
    pub common: CommonVmOpts,

    /// Configure logging for `bootc install` by setting the `RUST_LOG` environment variable.
    #[clap(long)]
    pub install_log: Option<String>,

    #[clap(
        long = "label",
        help = "Add metadata to the container in key=value form"
    )]
    pub label: Vec<String>,

    /// Check if the disk would be regenerated without actually creating it
    #[clap(long)]
    pub dry_run: bool,

    /// Pass an extra argument to the inner `podman run` that executes `bootc
    /// install`.  May be specified multiple times.  Useful for testing edge
    /// cases; for example `--bootc-install-podman-arg=--read-only` stresses
    /// the install path by making the container rootfs read-only, which
    /// exercises bootloader code paths that avoid writing to the host's
    /// read-only /boot (similar to osbuild sandbox environments).
    #[clap(long = "bootc-install-podman-arg")]
    pub bootc_install_podman_args: Vec<String>,

    /// Directory with systemd units to inject into the installer VM (expects
    /// system/ subdirectory with .service, .mount or .target units, like
    /// `ephemeral run --systemd-units`).  Units linked in
    /// system/default.target.wants/ are started during the install; units that
    /// change the sshd configuration need `Before=sshd.service`.  They only
    /// affect the installer VM, not the installed disk.
    #[clap(long = "systemd-units")]
    pub systemd_units_dir: Option<Utf8PathBuf>,
}

/// Configuration options for installing a bootc container image to disk
///
/// See the module-level documentation for details on the installation architecture and workflow.
#[derive(Debug, Parser)]
pub struct ToDiskOpts {
    /// Container image to install
    pub source_image: String,

    /// Target disk/device path
    pub target_disk: Utf8PathBuf,

    /// Installation options (filesystem, root-size, storage-path)
    #[clap(flatten)]
    pub install: InstallOptions,

    /// Additional installation options
    #[clap(flatten)]
    pub additional: ToDiskAdditionalOpts,
}

impl ToDiskOpts {
    /// Get the container image to use as the installation environment
    ///
    /// Uses the source image itself as the installer environment.
    fn get_installer_image(&self) -> &str {
        &self.source_image
    }

    /// Resolve and validate the container storage path
    ///
    /// Uses explicit storage_path if specified, otherwise auto-detects container storage.
    fn get_storage_path(&self) -> Result<Utf8PathBuf> {
        if let Some(ref path) = self.install.storage_path {
            utils::validate_container_storage_path(path)?;
            Ok(path.clone())
        } else {
            utils::detect_container_storage_path()
        }
    }

    /// Generate the complete bootc installation command arguments for SSH execution
    fn generate_bootc_install_command(&self, disk_size: u64) -> Result<Vec<String>> {
        let source_imgref = format!("containers-storage:{}", self.source_image);

        // Quote each bootc argument individually to prevent shell injection
        let mut quoted_bootc_args = Vec::new();
        for arg in self.install.to_bootc_args() {
            let quoted = shlex::try_quote(&arg)
                .map_err(|e| eyre!("Failed to quote bootc argument '{}': {}", arg, e))?;
            quoted_bootc_args.push(quoted.to_string());
        }
        let bootc_args = quoted_bootc_args.join(" ");

        // Quote the source image reference to prevent shell injection
        let quoted_source_imgref = shlex::try_quote(&source_imgref)
            .map_err(|e| eyre!("Failed to quote source imgref '{}': {}", source_imgref, e))?
            .to_string();

        // Quote the source image name for local storage operations
        let quoted_source_image = shlex::try_quote(&self.source_image)
            .map_err(|e| {
                eyre!(
                    "Failed to quote source image '{}': {}",
                    self.source_image,
                    e
                )
            })?
            .to_string();

        let install_log = self
            .additional
            .install_log
            .as_deref()
            .map(|v| shlex::try_quote(v))
            .transpose()?
            .map(|v| format!("--env=RUST_LOG={v}"))
            .unwrap_or_default();

        // Build extra podman args as a shell-safe string
        let extra_podman_args = self
            .additional
            .bootc_install_podman_args
            .iter()
            .map(|a| {
                shlex::try_quote(a)
                    .map(|q| q.to_string())
                    .map_err(|e| eyre!("Failed to quote podman arg '{}': {}", a, e))
            })
            .collect::<Result<Vec<_>>>()?
            .join(" ");

        // Size /var/tmp tmpfs to match swap size (disk_size)
        // This avoids duplicating size calculation logic
        let tmpfs_size_str = format!("size={}k", disk_size / 1024);
        let tmpfs_size_quoted = shlex::try_quote(&tmpfs_size_str)
            .map_err(|e| eyre!("Failed to quote tmpfs size: {}", e))?
            .to_string();

        // Create the complete script by substituting variables directly
        let script = indoc! {r#"
            set -euo pipefail

            echo "Setting up temporary filesystems..."
            # Mount /var/tmp as a large tmpfs, then symlink /var/lib/containers to it
            # to consolidate temporary storage in one location
            mount -t tmpfs -o {TMPFS_SIZE} tmpfs /var/tmp
            mkdir -p /var/tmp/containers
            rm /var/lib/containers -rf
            ln -sr /var/tmp/containers /var/lib/containers

            # Ensure virtiofs mount is available (fallback for older systemd without SMBIOS support)
            AIS=/run/virtiofs-mnt-hoststorage/
            if ! mountpoint -q ${AIS} &>/dev/null; then
                echo "virtiofs mount not found at ${AIS}, mounting manually..."
                mkdir -p ${AIS}
                mount -t virtiofs mount_hoststorage ${AIS} -o ro
            fi

            echo "Starting bootc installation..."
            echo "Source image: {SOURCE_IMGREF}"
            echo "Additional args: {BOOTC_ARGS}"

            tty=
            if test -t 0; then
                tty=--tty
            fi

            # Try bootc install first with the image from additionalimagestore
            # This avoids a deep copy when possible (issue #126)
            export STORAGE_OPTS=additionalimagestore=${AIS}

            # Execute bootc installation
            # Mount /var/tmp into inner container to avoid cross-device link errors (issue #125)
            set +e  # Don't exit on error, we'll check for signature error and retry
            ERROR_LOG=$(mktemp)
            podman run --rm -i ${tty} --privileged --pid=host --net=none -v /sys:/sys:ro \
                -v /var/lib/containers:/var/lib/containers -v /var/tmp:/var/tmp -v /dev:/dev -v "${AIS}:${AIS}" \
                --security-opt label=type:unconfined_t \
                --env=STORAGE_OPTS \
                {INSTALL_LOG} \
                {EXTRA_PODMAN_ARGS} \
                {SOURCE_IMGREF} \
                bootc install to-disk \
                --generic-image \
                --skip-fetch-check \
                {BOOTC_ARGS} \
                /dev/disk/by-id/virtio-output 2> "$ERROR_LOG"
            BOOTC_EXIT=$?
            set -e

            # Check if bootc failed with signature validation error
            if [ $BOOTC_EXIT -ne 0 ] && grep -q "Would invalidate signatures" "$ERROR_LOG"; then
                echo "Signature validation error detected, retrying with signature removal..."

                # Workaround for issue #126 and containers/image#2590:
                # Copy image to local storage without signatures when we hit the signature error.
                # This is a fallback - we only do the deep copy when necessary.
                # Note: containers/container-libs#144 would make this copy faster via reflinks.
                # The real fix is containers/image#2590 to carry signatures across representation changes.
                mkdir -p /etc/containers
                cat > /etc/containers/policy.json <<'EOF'
{
  "default": [{"type": "insecureAcceptAnything"}],
  "transports": {
    "containers-storage": {"": [{"type": "insecureAcceptAnything"}]},
    "docker": {"": [{"type": "insecureAcceptAnything"}]}
  }
}
EOF

                # Copy image without signatures
                skopeo copy --remove-signatures {SOURCE_IMGREF} containers-storage:{SOURCE_IMAGE}

                # Retry bootc install with the unsigned local copy
                podman run --rm -i ${tty} --privileged --pid=host --net=none -v /sys:/sys:ro \
                    -v /var/lib/containers:/var/lib/containers -v /var/tmp:/var/tmp -v /dev:/dev -v "${AIS}:${AIS}" \
                    --security-opt label=type:unconfined_t \
                    --env=STORAGE_OPTS \
                    {INSTALL_LOG} \
                    {EXTRA_PODMAN_ARGS} \
                    containers-storage:{SOURCE_IMAGE} \
                    bootc install to-disk \
                    --generic-image \
                    --skip-fetch-check \
                    {BOOTC_ARGS} \
                    /dev/disk/by-id/virtio-output
            elif [ $BOOTC_EXIT -ne 0 ]; then
                # Some other error, display it and fail
                cat "$ERROR_LOG" >&2
                rm -f "$ERROR_LOG"
                exit $BOOTC_EXIT
            fi

            rm -f "$ERROR_LOG"

            echo "Installation completed successfully!"
        "#}
        .replace("{TMPFS_SIZE}", &tmpfs_size_quoted)
        .replace("{SOURCE_IMGREF}", &quoted_source_imgref)
        .replace("{SOURCE_IMAGE}", &quoted_source_image)
        .replace("{INSTALL_LOG}", &install_log)
        .replace("{EXTRA_PODMAN_ARGS}", &extra_podman_args)
        .replace("{BOOTC_ARGS}", &bootc_args);

        Ok(vec!["/bin/bash".to_string(), "-c".to_string(), script])
    }

    /// Calculate the optimal target disk size based on the source image or explicit size
    ///
    /// Returns explicit disk_size if provided (parsed from human-readable format),
    /// otherwise 2x the image size with a 4GB minimum.
    fn calculate_disk_size(&self) -> Result<u64> {
        if let Some(ref size_str) = self.additional.disk_size {
            let parsed = utils::parse_size(size_str)?;
            debug!("Using explicit disk size: {} -> {} bytes", size_str, parsed);
            return Ok(parsed);
        }

        // Get the image size and multiply by 2 for installation space
        let image_size = images::get_image_size(&self.source_image)?;
        debug!("Image size for {}: {} bytes", self.source_image, image_size);

        // Minimum 4GB, otherwise 2x the image size
        let min_4gb = 4u64 * 1024 * 1024 * 1024;
        let disk_size = std::cmp::max(image_size * 2, min_4gb);
        debug!(
            "Calculated disk size: {} bytes (max({} * 2 = {}, {} min))",
            disk_size,
            image_size,
            image_size * 2,
            min_4gb
        );
        Ok(disk_size)
    }
}

/// Unit types copied by `--systemd-units`, matching `ephemeral run`.
const USER_UNIT_SUFFIXES: &[&str] = &[".service", ".mount", ".target"];

fn is_user_unit(name: &str) -> bool {
    USER_UNIT_SUFFIXES.iter().any(|s| name.ends_with(s))
}

/// Copy the user's `--systemd-units` directory into the installer VM's units directory.
///
/// Uses the layout of `ephemeral run --systemd-units`: unit files in `system/`,
/// enabled through `system/default.target.wants/`.  Returns the names of the
/// enabled units, which the caller adds to bcvk-to-disk.target; the wants
/// directory itself is not copied since default.target is never reached.
fn copy_user_units(src: &Utf8Path, dest_system: &std::path::Path) -> Result<Vec<String>> {
    let src_system = src.join("system");
    if !src_system.is_dir() {
        return Err(eyre!(
            "--systemd-units: {src_system} is not a directory (expects system/ subdirectory)"
        ));
    }

    for entry in src_system
        .read_dir_utf8()
        .with_context(|| format!("Reading {src_system}"))?
    {
        let entry = entry.with_context(|| format!("Reading {src_system}"))?;
        let name = entry.file_name();
        if !is_user_unit(name) {
            debug!("Skipping non-unit entry: {}", entry.path());
            continue;
        }
        let dest = dest_system.join(name);
        // A symlink to /dev/null masks a unit; an empty file does the same
        // and stays valid once the directory is mounted into the container.
        let is_mask =
            std::fs::read_link(entry.path()).is_ok_and(|t| t == Utf8Path::new("/dev/null"));
        if is_mask {
            std::fs::write(&dest, "")
        } else {
            std::fs::copy(entry.path(), &dest).map(|_| ())
        }
        .with_context(|| format!("Copying systemd unit {}", entry.path()))?;
        debug!("Copied systemd unit: {name}");
    }

    let src_wants = src_system.join("default.target.wants");
    if !src_wants.is_dir() {
        return Ok(Vec::new());
    }
    let mut wants = Vec::new();
    for entry in src_wants
        .read_dir_utf8()
        .with_context(|| format!("Reading {src_wants}"))?
    {
        let entry = entry.with_context(|| format!("Reading {src_wants}"))?;
        if is_user_unit(entry.file_name()) {
            wants.push(entry.file_name().to_owned());
        }
    }
    // Deterministic unit content regardless of directory order
    wants.sort();
    Ok(wants)
}

/// Generate bcvk-to-disk.target, additionally pulling in the given units.
fn to_disk_target_unit(extra_wants: &[String]) -> String {
    let mut unit = include_str!("units/bcvk-to-disk.target").to_string();
    if !extra_wants.is_empty() {
        unit.push_str(&format!("Wants={}\n", extra_wants.join(" ")));
    }
    unit
}

/// Outcome of a `run()` call, indicating whether a disk was created fresh
/// or reused from cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// A new disk image was created (or an existing one was regenerated).
    Created,
    /// An existing cached disk image was reused without reinstalling.
    Cached,
    /// Dry-run mode: the disk would have been reused.
    DryRunWouldReuse,
    /// Dry-run mode: the disk would have been regenerated.
    DryRunWouldRegenerate,
}

/// Execute a bootc installation using an ephemeral VM with SSH
///
/// Main entry point for the bootc installation process. See module-level documentation
/// for details on the installation workflow and architecture.
pub fn run(mut opts: ToDiskOpts) -> Result<RunOutcome> {
    // Normalize the source image name by stripping containers-storage: prefix if present.
    // The containers-storage: prefix is a transport specifier used by some tools like skopeo,
    // but podman commands (image inspect, run --mount=type=image) expect just the image name.
    // We'll add it back where needed (e.g., in bootc install commands).
    if let Some(stripped) = opts.source_image.strip_prefix("containers-storage:") {
        opts.source_image = stripped.to_string();
    }

    // Phase 0: Check for existing cached disk image
    let would_reuse = if opts.target_disk.exists() {
        debug!(
            "Target disk {} already exists, checking cache metadata",
            opts.target_disk
        );

        // Get the image digest for comparison
        let inspect = images::inspect(&opts.source_image)?;
        let image_digest = inspect.digest.to_string();

        // Check if cached disk matches our requirements
        match crate::cache_metadata::check_cached_disk(
            opts.target_disk.as_std_path(),
            &image_digest,
            &opts.source_image,
            &opts.install,
        )? {
            Ok(()) => {
                if opts.additional.dry_run {
                    return Ok(RunOutcome::DryRunWouldReuse);
                }
                return Ok(RunOutcome::Cached);
            }
            Err(e) => {
                debug!("Existing disk does not match requirements, recreating: {e}");
                if !opts.additional.dry_run {
                    // Remove the existing disk so we can recreate it
                    std::fs::remove_file(&opts.target_disk).with_context(|| {
                        format!("Failed to remove existing disk {}", opts.target_disk)
                    })?;
                }
                false
            }
        }
    } else {
        false
    };

    // In dry-run mode, report what would happen without doing it
    if opts.additional.dry_run {
        return if would_reuse {
            Ok(RunOutcome::DryRunWouldReuse)
        } else {
            Ok(RunOutcome::DryRunWouldRegenerate)
        };
    }

    // Phase 1: Validation and preparation
    // Resolve container storage path (auto-detect or validate specified path)
    let storage_path = opts.get_storage_path()?;

    // Debug logging for installation configuration
    if opts.additional.common.debug {
        debug!("Using container storage: {:?}", storage_path);
        debug!("Installing to target disk: {:?}", opts.target_disk);
        debug!("Filesystem: {:?}", opts.install.filesystem);
        if let Some(ref root_size) = opts.install.root_size {
            debug!("Root size: {}", root_size);
        }
    }

    let disk_size = opts.calculate_disk_size()?;

    // Create disk image based on format
    match opts.additional.format {
        Format::Raw => {
            // Create sparse file - only allocates space as data is written
            let file = std::fs::File::create(&opts.target_disk)
                .with_context(|| format!("Opening {}", opts.target_disk))?;
            file.set_len(disk_size)?;
            // TODO pass to qemu via fdset
            drop(file);
        }
        Format::Qcow2 => {
            // Use qemu-img to create qcow2 format
            debug!("Creating qcow2 with size {} bytes", disk_size);
            let size_arg = disk_size.to_string();
            let output = std::process::Command::new("qemu-img")
                .args([
                    "create",
                    "-f",
                    "qcow2",
                    opts.target_disk.as_str(),
                    &size_arg,
                ])
                .output()
                .with_context(|| {
                    format!("Failed to run qemu-img create for {}", opts.target_disk)
                })?;

            if !output.status.success() {
                return Err(color_eyre::eyre::eyre!(
                    "qemu-img create failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            debug!("qemu-img create completed successfully");
        }
    }

    // Phase 3: Installation command generation
    // Generate complete script including storage setup and bootc install
    let bootc_install_command = opts.generate_bootc_install_command(disk_size)?;

    // Phase 4: Ephemeral VM configuration
    let mut common_opts = opts.additional.common.clone();
    // Enable SSH key generation for SSH-based installation
    common_opts.ssh_keygen = true;

    let tty = std::io::stdout().is_terminal();

    // Boot the install VM to a minimal target instead of default.target.
    // This is to prevent user-created services in the image (such as quadlets)
    // from interfering with the to-disk install process.
    //
    // bcvk-to-disk.target starts only what the install needs:
    // basic.target, network-online + sshd (the host drives
    // the install over SSH), and remote-fs (virtiofs host-storage mounts).
    //
    // User units from --systemd-units are copied alongside it. Since
    // default.target is never reached, the units they enable via
    // default.target.wants/ are pulled in by bcvk-to-disk.target instead.
    let units_dir = TempDir::new()?;
    let units_system_dir = units_dir.path().join("system");
    std::fs::create_dir_all(&units_system_dir)?;
    let user_wants = match opts.additional.systemd_units_dir {
        Some(ref dir) => copy_user_units(dir, &units_system_dir)?,
        None => Vec::new(),
    };
    std::fs::write(
        units_system_dir.join("bcvk-to-disk.target"),
        to_disk_target_unit(&user_wants),
    )?;

    // Configure VM for installation:
    // - Use source image as installer environment
    // - Mount host storage read-only for image access
    // - Attach target disk via virtio-blk
    // - Boot to bcvk-to-disk.target instead of default.target
    let ephemeral_opts = RunEphemeralOpts {
        host_dns_servers: None,
        image: opts.get_installer_image().to_string(),
        common: common_opts,
        podman: crate::run_ephemeral::CommonPodmanOptions {
            rm: true,     // Clean up container after installation
            detach: true, // Run in detached mode for SSH approach
            tty,
            label: opts.additional.label,
            ..Default::default()
        },
        // Workaround for https://github.com/containers/container-libs/issues/144#issuecomment-3300424410
        // Basically containers-libs allocates a tempfile for a whole serialization of a layer as a tarball
        // when fetching, so we need enough memory to do so.
        add_swap: Some(format!("{disk_size}")),
        bind_mounts: Vec::new(),    // No additional bind mounts needed
        ro_bind_mounts: Vec::new(), // No additional ro bind mounts needed
        systemd_units_dir: Some(units_dir.path().to_string_lossy().to_string()),
        bind_storage_ro: true, // Mount host container storage read-only
        mount_disk_files: vec![format!(
            "{}:output:{}",
            opts.target_disk,
            opts.additional.format.as_str()
        )], // Attach target disk
        tpm: false,
        kernel_args: vec!["systemd.unit=bcvk-to-disk.target".to_string()],
        ignition_config: None,
        debug_entrypoint: None,
    };

    // Phase 5: SSH-based VM configuration and execution
    // Launch VM in detached mode with SSH enabled
    debug!("Starting ephemeral VM with SSH...");
    let container_id = run_detached(ephemeral_opts)?;
    debug!("Ephemeral VM started with container ID: {}", container_id);

    // Use the SSH approach for better TTY forwarding and output buffering
    let result = (|| -> Result<()> {
        // Wait for SSH to be ready
        let progress_bar = crate::boot_progress::create_boot_progress_bar();
        let (duration, progress_bar) = wait_for_ssh_ready(&container_id, None, progress_bar)?;
        progress_bar.finish_and_clear();
        tracing::info!(
            "Connected ({} elapsed), beginning installation...",
            HumanDuration(duration)
        );

        // Connect via SSH and execute the installation command
        debug!(
            "Executing installation via SSH: {:?}",
            bootc_install_command
        );
        let ssh_options = ssh::SshConnectionOptions {
            allocate_tty: tty,
            ..ssh::SshConnectionOptions::default()
        };
        let status = ssh::connect(&container_id, bootc_install_command, &ssh_options)?;
        if !status.success() {
            return Err(eyre!(
                "SSH installation command failed with exit code: {:?}",
                status.code()
            ));
        }

        Ok(())
    })();

    // Cleanup: stop and remove the container
    debug!("Cleaning up ephemeral container...");
    let _ = std::process::Command::new("podman")
        .args(["rm", "-f", "--", &container_id])
        .output();

    // Handle the result - remove disk file on failure
    match result {
        Ok(()) => {
            // Write metadata to the disk image for caching
            // Extract values before they're potentially moved
            let write_result = write_disk_metadata(
                &opts.source_image,
                &opts.target_disk,
                &opts.install,
                &opts.additional.format,
            );
            if let Err(e) = write_result {
                debug!("Failed to write metadata to disk image: {}", e);
                // Don't fail the operation just because metadata couldn't be written
            }
            Ok(RunOutcome::Created)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&opts.target_disk);
            Err(e)
        }
    }
}

/// Write metadata to disk image for caching purposes
fn write_disk_metadata(
    source_image: &str,
    target_disk: &Utf8PathBuf,
    install_options: &InstallOptions,
    format: &Format,
) -> Result<()> {
    // Note: xattrs work on regular files including raw and qcow2 images
    // as they're stored in the filesystem metadata, not inside the disk image

    // Get the image digest
    let inspect = images::inspect(source_image)?;
    let digest = inspect.digest.to_string();

    // Prepare metadata using the new helper method
    let metadata = DiskImageMetadata::from(install_options, &digest, source_image);

    // Write metadata using rustix fsetxattr
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(target_disk)
        .with_context(|| format!("Failed to open disk file {}", target_disk))?;

    metadata
        .write_to_file(&file)
        .with_context(|| "Failed to write metadata to disk file")?;

    debug!(
        "Successfully wrote cache metadata to disk image for format {:?}",
        format
    );
    Ok(())
}

// Note: Unit tests should not launch containers, VMs, or perform other system-level operations.
// Integration tests that launch containers/VMs should be placed in the integration-tests crate.
// Unit tests here should only test pure functions and basic validation logic.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_disk_size() -> Result<()> {
        // Test with explicit disk size
        let opts = ToDiskOpts {
            source_image: "test:latest".to_string(),
            target_disk: "/tmp/test.img".into(),
            install: InstallOptions {
                filesystem: Some("ext4".to_string()),
                ..Default::default()
            },
            additional: ToDiskAdditionalOpts {
                disk_size: Some("10G".to_string()),
                ..Default::default()
            },
        };

        let size = opts.calculate_disk_size()?;
        // Should be 10GB as specified
        assert_eq!(size, 10 * 1024 * 1024 * 1024);

        // Test with another size format
        let opts2 = ToDiskOpts {
            source_image: "test:latest".to_string(),
            target_disk: "/tmp/test.img".into(),
            install: InstallOptions {
                filesystem: Some("ext4".to_string()),
                ..Default::default()
            },
            additional: ToDiskAdditionalOpts {
                disk_size: Some("5120M".to_string()),
                ..Default::default()
            },
        };

        let size2 = opts2.calculate_disk_size()?;
        assert_eq!(size2, 5120 * 1024 * 1024);

        Ok(())
    }

    /// Clap parses `--flag=--value` by treating everything after `=` as the raw
    /// value, so `--bootc-install-podman-arg=--read-only` works without needing
    /// `allow_hyphen_values = true`.  This test documents and locks that behaviour.
    #[test]
    fn test_podman_arg_equals_form_accepts_hyphen_values() {
        use clap::Parser;

        let opts = ToDiskOpts::try_parse_from([
            "bcvk",
            "test:latest",
            "/tmp/test.img",
            "--bootc-install-podman-arg=--read-only",
            "--bootc-install-podman-arg=--env=FOO=bar",
        ])
        .expect("--flag=--value form must parse without allow_hyphen_values");

        assert_eq!(
            opts.additional.bootc_install_podman_args,
            ["--read-only", "--env=FOO=bar"]
        );
    }

    #[test]
    fn test_to_disk_target_unit() {
        let base = include_str!("units/bcvk-to-disk.target");
        let cases: &[(&[&str], &str)] = &[
            (&[], ""),
            (&["a.service"], "Wants=a.service\n"),
            (&["a.service", "b.mount"], "Wants=a.service b.mount\n"),
        ];
        for (wants, extra) in cases {
            let wants: Vec<String> = wants.iter().map(|w| w.to_string()).collect();
            assert_eq!(to_disk_target_unit(&wants), format!("{base}{extra}"));
        }
    }

    #[test]
    fn test_copy_user_units() -> Result<()> {
        /// An entry in the user's units directory, relative to its root
        enum Src {
            File(&'static str, &'static str),
            Link(&'static str, &'static str),
        }
        use Src::*;

        struct Case {
            name: &'static str,
            src: &'static [Src],
            /// Enabled units, or None if an error is expected
            wants: Option<&'static [&'static str]>,
            /// Files expected in the destination, with their content
            copied: &'static [(&'static str, &'static str)],
            /// Names expected to be absent from the destination
            skipped: &'static [&'static str],
        }

        let cases = [
            Case {
                name: "missing system/ subdirectory",
                src: &[File("a.service", "[Unit]\n")],
                wants: None,
                copied: &[],
                skipped: &[],
            },
            Case {
                name: "units without default.target.wants/",
                src: &[
                    File("system/a.service", "[Unit]\n"),
                    File("system/b.mount", "[Mount]\n"),
                    File("system/c.target", "[Unit]\n"),
                ],
                wants: Some(&[]),
                copied: &[
                    ("a.service", "[Unit]\n"),
                    ("b.mount", "[Mount]\n"),
                    ("c.target", "[Unit]\n"),
                ],
                skipped: &["default.target.wants"],
            },
            Case {
                name: "enabled units are sorted, non-units skipped",
                src: &[
                    File("system/a.service", "[Unit]\n"),
                    File("system/b.service", "[Unit]\n"),
                    File("system/README", ""),
                    File("system/c.socket", "[Socket]\n"),
                    Link("system/default.target.wants/b.service", "../b.service"),
                    Link("system/default.target.wants/a.service", "../a.service"),
                    // Units shipped in the image can be enabled too
                    Link(
                        "system/default.target.wants/image.service",
                        "/usr/lib/systemd/system/image.service",
                    ),
                    File("system/default.target.wants/.gitkeep", ""),
                ],
                wants: Some(&["a.service", "b.service", "image.service"]),
                copied: &[("a.service", "[Unit]\n"), ("b.service", "[Unit]\n")],
                skipped: &["README", "c.socket", "default.target.wants"],
            },
            Case {
                name: "masked unit becomes an empty file",
                src: &[Link("system/masked.service", "/dev/null")],
                wants: Some(&[]),
                copied: &[("masked.service", "")],
                skipped: &[],
            },
        ];

        for case in cases {
            let src = TempDir::new()?;
            let src = Utf8Path::from_path(src.path()).unwrap();
            for entry in case.src {
                let (File(path, _) | Link(path, _)) = entry;
                let path = src.join(path);
                std::fs::create_dir_all(path.parent().unwrap())?;
                match entry {
                    File(_, content) => std::fs::write(&path, content)?,
                    Link(_, target) => std::os::unix::fs::symlink(target, &path)?,
                }
            }

            let dest = TempDir::new()?;
            let result = copy_user_units(src, dest.path());
            let Some(expected_wants) = case.wants else {
                assert!(result.is_err(), "{}: expected an error", case.name);
                continue;
            };
            assert_eq!(result?, expected_wants, "{}", case.name);
            for (name, content) in case.copied {
                let path = dest.path().join(name);
                assert!(!path.is_symlink(), "{}: {name} is a symlink", case.name);
                assert_eq!(std::fs::read_to_string(&path)?, *content, "{}", case.name);
            }
            for name in case.skipped {
                assert!(
                    !dest.path().join(name).exists(),
                    "{}: {name} should not be copied",
                    case.name
                );
            }
        }
        Ok(())
    }
}
