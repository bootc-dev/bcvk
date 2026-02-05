//! Upload bootc disk images to libvirt with proper metadata annotations
//!
//! This module provides functionality to upload disk images created by to-disk
//! to libvirt storage pools, maintaining container image metadata as libvirt annotations.

use crate::common_opts::MemoryOpts;
use crate::images;
use crate::install_options::InstallOptions;
use crate::to_disk::{run as to_disk, ToDiskAdditionalOpts, ToDiskOpts};
use crate::utils::DiskSize;
use camino::Utf8PathBuf;
use clap::Parser;
use color_eyre::{eyre::eyre, Result};
use std::path::Path;
use std::process::Command;
use tracing::debug;

/// Configuration options for uploading a bootc disk image to libvirt
#[derive(Debug, Parser, Clone)]
pub struct LibvirtUploadOpts {
    /// Container image to install and upload
    pub source_image: String,

    /// Name for the libvirt volume (defaults to sanitized image name)
    #[clap(long)]
    pub volume_name: Option<String>,

    /// Libvirt storage pool name
    #[clap(long, default_value = "default")]
    pub pool: String,

    /// Size of the disk image (e.g., '20G', '10240M'). If not specified, uses the actual size of the created disk.
    #[clap(long)]
    pub disk_size: Option<DiskSize>,

    /// Installation options (filesystem, root-size, storage-path)
    #[clap(flatten)]
    pub install: InstallOptions,

    #[clap(flatten)]
    pub memory: MemoryOpts,

    /// Number of vCPUs for installation VM
    #[clap(long)]
    pub vcpus: Option<u32>,
}

impl LibvirtUploadOpts {
    /// Build a virsh command with optional connection URI  
    fn virsh_command(&self, global_opts: &crate::libvirt::LibvirtOptions) -> Command {
        global_opts.virsh_command()
    }

    /// Generate a sanitized volume name from the container image
    pub fn get_volume_name(&self) -> String {
        if let Some(ref name) = self.volume_name {
            return name.clone();
        }

        // Sanitize the image name for use as a volume name
        let image_name = self.source_image.clone();

        // Remove registry prefix if present
        let name = image_name
            .split('/')
            .last()
            .unwrap_or(&image_name)
            .replace(':', "-")
            .replace('/', "-")
            .replace('.', "-");

        format!("bootc-{}", name)
    }

    /// Generate a volume name that includes the container digest for caching
    pub fn get_cached_volume_name(&self, image_digest: &str) -> String {
        if let Some(ref name) = self.volume_name {
            return name.clone();
        }

        // Create a name that includes digest for caching
        let base_name = self.get_volume_name();
        // Take the first 12 chars of the digest (after sha256:)
        let digest_short = image_digest.strip_prefix("sha256:").unwrap_or(image_digest);
        let digest_short = &digest_short[..std::cmp::min(12, digest_short.len())];

        format!("{}-{}", base_name, digest_short)
    }

    /// Create a temporary file path for the disk image
    /// Returns a temporary directory and the disk path within it.
    /// The directory ensures cleanup when dropped, and the disk path doesn't exist yet.
    fn get_temp_disk_path(&self) -> Result<(tempfile::TempDir, Utf8PathBuf)> {
        let temp_dir = tempfile::Builder::new()
            .prefix("bcvk-libvirt-upload")
            .tempdir()?;
        let disk_path = temp_dir.path().join("disk.img").try_into().unwrap();
        Ok((temp_dir, disk_path))
    }

    /// Check if libvirt storage pool exists
    fn check_pool_exists(&self, global_opts: &crate::libvirt::LibvirtOptions) -> Result<()> {
        let output = self
            .virsh_command(global_opts)
            .args(&["pool-info", &self.pool])
            .output()?;

        if !output.status.success() {
            return Err(eyre!(
                "Storage pool '{}' does not exist. Create it with: virsh pool-define-as {} dir - - - - /var/lib/libvirt/images",
                self.pool, self.pool
            ));
        }

        Ok(())
    }

    /// Upload the disk image to libvirt storage pool
    fn upload_to_libvirt(
        &self,
        global_opts: &crate::libvirt::LibvirtOptions,
        disk_path: &Path,
        disk_size_bytes: u64,
        image_digest: &str,
    ) -> Result<()> {
        debug!("Uploading disk to libvirt pool '{}'", self.pool);

        // Check pool exists
        self.check_pool_exists(global_opts)?;

        let volume_name = self.get_cached_volume_name(image_digest);
        let volume_path = format!("{}.raw", volume_name);

        // Delete existing volume if it exists
        let _ = self
            .virsh_command(global_opts)
            .args(&["vol-delete", &volume_path, "--pool", &self.pool])
            .output();

        // Use the provided disk size
        let output = self
            .virsh_command(global_opts)
            .args(&[
                "vol-create-as",
                &self.pool,
                &volume_path,
                &disk_size_bytes.to_string(),
                "--format",
                "raw",
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!("Failed to create volume: {}", stderr));
        }

        // Upload the disk image to the volume
        debug!("Uploading disk image to volume '{}'", volume_path);
        let output = self
            .virsh_command(global_opts)
            .args(&[
                "vol-upload",
                &volume_path,
                disk_path.to_str().unwrap(),
                "--pool",
                &self.pool,
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!("Failed to upload volume: {}", stderr));
        }

        Ok(())
    }
}

/// Execute the libvirt disk upload process
pub fn run(global_opts: &crate::libvirt::LibvirtOptions, opts: LibvirtUploadOpts) -> Result<()> {
    debug!(
        "Starting libvirt disk upload for image: {}",
        opts.source_image
    );

    // Phase 1: Extract image digest for caching
    let inspect = images::inspect(&opts.source_image)?;
    let image_digest = &inspect.digest.to_string();
    debug!("Container image digest: {}", image_digest);

    // Phase 2: Calculate disk size to use
    let disk_size = if let Some(size) = opts.disk_size {
        // Use explicit size if provided
        size
    } else {
        // Use same logic as to_disk: 2x source image size with 4GB minimum
        let image_size = images::get_image_size(&opts.source_image)?;

        DiskSize::from_bytes(std::cmp::max(image_size * 2, 4u64 * 1024 * 1024 * 1024))
    };

    // Phase 2: Create temporary disk path
    let (temp_dir, temp_disk_path) = opts.get_temp_disk_path()?;
    debug!("Using temporary disk: {:?}", temp_disk_path);

    // Phase 3: Run installation to create disk image
    debug!("Running bootc installation to create disk image");

    let install_opts = ToDiskOpts {
        source_image: opts.source_image.clone(),
        target_disk: temp_disk_path.clone(),
        install: opts.install.clone(),
        additional: ToDiskAdditionalOpts {
            disk_size: Some(disk_size),
            common: crate::run_ephemeral::CommonVmOpts {
                memory: opts.memory.clone(),
                vcpus: opts.vcpus,
                ..Default::default()
            },
            ..Default::default()
        },
    };

    to_disk(install_opts)?;

    opts.upload_to_libvirt(
        global_opts,
        temp_disk_path.as_std_path(),
        disk_size.as_bytes(),
        &image_digest,
    )?;

    // Keep temp_dir alive until upload completes to prevent cleanup
    drop(temp_dir);

    let volume_name = opts.get_cached_volume_name(&image_digest);
    debug!(
        "Successfully uploaded disk as volume '{}' to pool '{}'",
        volume_name, opts.pool
    );
    debug!("Container image annotation added: {}", opts.source_image);
    Ok(())
}
