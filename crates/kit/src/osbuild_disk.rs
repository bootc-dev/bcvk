//! Build disk images using bootc-image-builder via ephemeral VMs
//!
//! This module provides functionality to build bootc container disk images using
//! bootc-image-builder (b-i-b) through an ephemeral VM-based approach, mirroring
//! the architecture of the to-disk module.
//!
//! # Build Workflow
//!
//! The bootc-image-builder build process follows these key steps:
//!
//! 1. **Output Directory Setup**: Validates and creates the output directory where
//!    disk images will be stored
//!
//! 2. **Storage Configuration**: Mounts the host's container storage read-only to
//!    provide b-i-b access to the source container image without network dependencies
//!
//! 3. **Ephemeral VM Launch**: Creates a temporary VM with:
//!    - Host container storage mounted read-only at /run/virtiofs-mnt-hoststorage
//!    - Output directory mounted writable at /run/virtiofs-mnt-output
//!
//! 4. **B-I-B Execution**: Runs bootc-image-builder container inside the VM with
//!    STORAGE_OPTS configured to use the host storage as an additional image store
//!
//! 5. **Output Collection**: Built disk images are written to the output directory
//!    via the writable VirtioFS mount
//!
//! 6. **Cleanup**: The ephemeral VM automatically shuts down after the build,
//!    leaving the disk images in the output directory
//!
//! # VirtioFS Architecture
//!
//! The module uses VirtioFS for filesystem sharing:
//!
//! - **Host Storage (read-only)**: Mounted at /run/virtiofs-mnt-hoststorage in the VM
//!   to provide b-i-b access to the source container image
//!
//! - **Output Directory (writable)**: Mounted at /run/virtiofs-mnt-output in the VM
//!   where b-i-b writes the generated disk images
//!
//! # B-I-B Container Invocation
//!
//! The b-i-b container is invoked inside the VM using podman with:
//! - Privileged mode for disk operations
//! - Host storage mounted read-only
//! - Output directory mounted writable
//! - STORAGE_OPTS environment variable pointing to host storage
//!
//! # Usage Examples
//!
//! ```bash
//! # Build a qcow2 image (default)
//! bcvk osbuild-disk quay.io/centos-bootc/centos-bootc:stream10 ./output
//!
//! # Build multiple image types
//! bcvk osbuild-disk --type qcow2 --type ami \
//!     quay.io/centos-bootc/centos-bootc:stream10 ./output
//!
//! # Use custom b-i-b image
//! bcvk osbuild-disk --bib-image quay.io/my-org/bootc-image-builder:latest \
//!     quay.io/centos-bootc/centos-bootc:stream10 ./output
//! ```

use std::io::IsTerminal;

use crate::common_opts::MemoryOpts;
use crate::install_options::InstallOptions;
use crate::run_ephemeral::{run_detached, CommonVmOpts, RunEphemeralOpts};
use crate::run_ephemeral_ssh::wait_for_ssh_ready;
use crate::{ssh, utils};
use camino::Utf8PathBuf;
use clap::Parser;
use color_eyre::eyre::{eyre, Context};
use color_eyre::Result;
use indicatif::HumanDuration;
use indoc::indoc;
use tracing::debug;

/// Configuration options for building disk images with bootc-image-builder
///
/// See the module-level documentation for details on the build architecture and workflow.
#[derive(Debug, Parser)]
pub struct OsbuildDiskOpts {
    /// Container image to build from
    pub source_image: String,

    /// Output directory for disk images
    pub output_dir: Utf8PathBuf,

    /// Image types to build (e.g., qcow2, ami, vmdk, iso)
    #[clap(long = "type", default_value = "qcow2")]
    pub image_types: Vec<String>,

    /// Optional b-i-b config file
    #[clap(long)]
    pub config_file: Option<Utf8PathBuf>,

    /// Root filesystem type (xfs, ext4, btrfs)
    #[clap(long)]
    pub rootfs: Option<String>,

    /// B-I-B container image to use
    #[clap(
        long,
        default_value = "quay.io/centos-bootc/bootc-image-builder:latest"
    )]
    pub bib_image: String,

    /// Add metadata to the container in key=value form
    #[clap(long = "label")]
    pub label: Vec<String>,

    /// Installation options (filesystem, root-size, storage-path)
    #[clap(flatten)]
    pub install: InstallOptions,

    /// Common VM configuration options
    #[clap(flatten)]
    pub common: CommonVmOpts,
}

impl OsbuildDiskOpts {
    /// Get the container image to use as the VM environment
    ///
    /// Uses the source image as the VM environment (same pattern as to-disk).
    /// The b-i-b container will run inside this VM.
    fn get_vm_image(&self) -> &str {
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

    /// Generate the complete b-i-b command for SSH execution
    ///
    /// If config_in_vm is provided, it's the path to the config file in the VM
    /// that should be mounted into the b-i-b container.
    fn generate_bib_command(&self, config_in_vm: Option<&str>) -> Result<Vec<String>> {
        let source_imgref = format!("containers-storage:{}", self.source_image);
        let source_image = &self.source_image;

        // Build the image type arguments
        let types_arg = self.image_types.join(",");
        let quoted_types = shlex::try_quote(&types_arg)
            .map_err(|e| eyre!("Failed to quote types argument '{}': {}", types_arg, e))?
            .to_string();

        // Quote the source image reference
        let quoted_source_imgref = shlex::try_quote(&source_imgref)
            .map_err(|e| eyre!("Failed to quote source imgref '{}': {}", source_imgref, e))?
            .to_string();

        // Quote the b-i-b image
        let quoted_bib_image = shlex::try_quote(&self.bib_image)
            .map_err(|e| eyre!("Failed to quote bib image '{}': {}", self.bib_image, e))?
            .to_string();

        // Build optional arguments and volume mounts
        let mut optional_args = Vec::new();
        let mut volume_mounts = Vec::new();

        // Handle config file mounting if provided
        if let Some(config_path) = config_in_vm {
            let quoted_config = shlex::try_quote(config_path)
                .map_err(|e| eyre!("Failed to quote config file '{}': {}", config_path, e))?
                .to_string();
            // Mount the config file from VM into b-i-b container at /config
            volume_mounts.push(format!("-v {}:/config:ro", quoted_config));
            optional_args.push("--config /config".to_string());
        }

        if let Some(ref rootfs) = self.rootfs {
            let quoted_rootfs = shlex::try_quote(rootfs)
                .map_err(|e| eyre!("Failed to quote rootfs '{}': {}", rootfs, e))?
                .to_string();
            optional_args.push(format!("--rootfs {}", quoted_rootfs));
        }

        let optional_args_str = optional_args.join(" ");
        let volume_mounts_str = volume_mounts.join(" \\\n                ");

        // Create the complete script
        let script = indoc! {r#"
            set -euo pipefail

            echo "Starting bootc-image-builder..."
            echo "Source image: {SOURCE_IMGREF}"
            echo "Output directory: /run/virtiofs-mnt-output"
            echo "Image types: {TYPES}"

            # Set up container storage in VM
            # Mount tmpfs directly at /var/lib/containers to avoid overlay-on-overlay issues
            # Don't use symlink to avoid database path mismatches with b-i-b
            # Use 40GB to handle temporary copies during container-deploy stage
            echo "Setting up VM container storage..."
            rm -rf /var/lib/containers
            mkdir -p /var/lib/containers
            mount -t tmpfs -o size=40G tmpfs /var/lib/containers

            # Configure VM's podman to use host storage as additional image store
            # This allows skopeo to read from the host storage for copying
            AIS=/run/virtiofs-mnt-hoststorage
            export STORAGE_OPTS=additionalimagestore=${AIS}

            # Pre-copy source image for b-i-b manifest generation and osbuild stages
            # Export to OCI directory which both b-i-b and osbuild stages can use
            echo "Preparing source image..."
            mkdir -p /var/lib/containers/exported
            # Export to OCI directory
            env STORAGE_OPTS="additionalimagestore=${AIS}" skopeo copy {SOURCE_IMGREF} oci:/var/lib/containers/exported/image

            tty=
            if test -t 0; then
                tty=--tty
            fi

            # Execute b-i-b with pre-populated containers-storage
            # B-i-b requires containers-storage access, so we copy the OCI image into
            # the b-i-b container's own storage before running b-i-b
            # Write output directly to virtiofs mount (now properly writable)
            echo "Running bootc-image-builder..."
            podman run --rm -i ${tty} --privileged \
                --security-opt label=type:unconfined_t \
                -v /var/lib/containers/exported:/var/lib/containers/exported:ro \
                -v /run/virtiofs-mnt-output:/output \
                {VOLUME_MOUNTS} \
                --entrypoint /bin/bash \
                {BIB_IMAGE} \
                -c "skopeo copy oci:/var/lib/containers/exported/image containers-storage:{SOURCE_IMAGE} && exec bootc-image-builder --type {TYPES} {OPTIONAL_ARGS} {SOURCE_IMAGE}"

            echo "Build completed successfully!"
        "#}
        .replace("{SOURCE_IMGREF}", &quoted_source_imgref)
        .replace("{SOURCE_IMAGE}", source_image)
        .replace("{TYPES}", &quoted_types)
        .replace("{BIB_IMAGE}", &quoted_bib_image)
        .replace("{OPTIONAL_ARGS}", &optional_args_str)
        .replace(
            "{VOLUME_MOUNTS}",
            if volume_mounts_str.is_empty() {
                ""
            } else {
                &volume_mounts_str
            },
        );

        Ok(vec!["/bin/bash".to_string(), "-c".to_string(), script])
    }
}

/// Execute a bootc-image-builder build using an ephemeral VM with SSH
///
/// Main entry point for the b-i-b build process. See module-level documentation
/// for details on the build workflow and architecture.
pub fn run(opts: OsbuildDiskOpts) -> Result<()> {
    // Phase 1: Validation and preparation
    // Resolve container storage path (auto-detect or validate specified path)
    let storage_path = opts.get_storage_path()?;

    // Create output directory if it doesn't exist
    std::fs::create_dir_all(&opts.output_dir)
        .with_context(|| format!("Failed to create output directory {}", opts.output_dir))?;

    // Convert output directory to absolute path for mounting
    let output_dir_absolute = if opts.output_dir.is_absolute() {
        opts.output_dir.clone()
    } else {
        let canonical = opts.output_dir.canonicalize()?;
        Utf8PathBuf::try_from(canonical)?
    };

    // Process config file if provided
    let (config_file_absolute, config_in_vm) = if let Some(ref config) = opts.config_file {
        // Validate config file exists
        if !config.exists() {
            return Err(eyre!("Config file does not exist: {}", config));
        }

        // Convert to absolute path
        let config_absolute = if config.is_absolute() {
            config.clone()
        } else {
            let canonical = config
                .canonicalize()
                .with_context(|| format!("Failed to canonicalize config file {}", config))?;
            Utf8PathBuf::try_from(canonical)?
        };

        // Extract filename to construct VM path
        let filename = config_absolute
            .file_name()
            .ok_or_else(|| eyre!("Config file path has no filename: {}", config_absolute))?;
        let vm_path = format!("/run/virtiofs-mnt-bibconfig/{}", filename);

        (Some(config_absolute), Some(vm_path))
    } else {
        (None, None)
    };

    // Debug logging for build configuration
    if opts.common.debug {
        debug!("Using container storage: {:?}", storage_path);
        debug!("Output directory: {:?}", output_dir_absolute);
        debug!("Image types: {:?}", opts.image_types);
        debug!("B-I-B image: {}", opts.bib_image);
        if let Some(ref cfg) = config_file_absolute {
            debug!("Config file: {:?} -> {:?}", cfg, config_in_vm);
        }
    }

    // Phase 2: Build command generation
    let bib_command = opts.generate_bib_command(config_in_vm.as_deref())?;

    // Phase 3: Ephemeral VM configuration
    let mut common_opts = opts.common.clone();
    // Enable SSH key generation for SSH-based execution
    common_opts.ssh_keygen = true;
    common_opts.memory = MemoryOpts {
        memory: "20G".to_string(),
    };

    let tty = std::io::stdout().is_terminal();

    // Configure VM for b-i-b execution:
    // - Use b-i-b image as VM environment
    // - Mount host storage read-only for image access
    // - Mount output directory writable for build artifacts
    // - Mount config file read-only if provided
    // - Disable networking (using local storage only)
    let bind_mounts = vec![format!("{}:output", output_dir_absolute)];
    let mut ro_bind_mounts = Vec::new();

    // Add config file mount if provided
    if let Some(ref config_path) = config_file_absolute {
        ro_bind_mounts.push(format!("{}:bibconfig", config_path));
    }

    let ephemeral_opts = RunEphemeralOpts {
        image: opts.get_vm_image().to_string(),
        common: common_opts,
        podman: crate::run_ephemeral::CommonPodmanOptions {
            rm: true,     // Clean up container after build
            detach: true, // Run in detached mode for SSH approach
            tty,
            label: opts.label.clone(),
            ..Default::default()
        },
        bind_mounts,
        ro_bind_mounts,
        systemd_units_dir: None,
        bind_storage_ro: true, // Mount host container storage read-only
        add_swap: None,
        mount_disk_files: vec![],
        kernel_args: vec![],
    };

    // Phase 4: SSH-based VM configuration and execution
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
        println!(
            "Connected ({} elapsed), beginning build...",
            HumanDuration(duration)
        );

        // Connect via SSH and execute the b-i-b command
        debug!("Executing b-i-b via SSH: {:?}", bib_command);
        let ssh_options = ssh::SshConnectionOptions {
            allocate_tty: tty,
            ..ssh::SshConnectionOptions::default()
        };
        let status = ssh::connect(&container_id, bib_command, &ssh_options)?;
        if !status.success() {
            return Err(eyre!(
                "B-I-B build command failed with exit code: {:?}",
                status.code()
            ));
        }

        println!("Build artifacts written to: {}", output_dir_absolute);
        Ok(())
    })();

    // Cleanup: stop and remove the container
    debug!("Cleaning up ephemeral container...");
    let _ = std::process::Command::new("podman")
        .args(["rm", "-f", &container_id])
        .output();

    // Return the result
    result?;
    println!("Build completed successfully!");
    println!("Output directory: {}", output_dir_absolute);
    Ok(())
}
