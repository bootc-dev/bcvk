//! Upload bootc disk images to libvirt with proper metadata annotations
//!
//! This module provides functionality to upload disk images created by to-disk
//! to libvirt storage pools, maintaining container image metadata as libvirt annotations.

use crate::common_opts::MemoryOpts;
use crate::images;
use crate::install_options::InstallOptions;
use crate::to_disk::{run as to_disk, ToDiskAdditionalOpts, ToDiskOpts};
use crate::utils::DiskSize;
use crate::xml_utils::{self, XmlWriter};
use camino::Utf8Path;
use clap::Parser;
use color_eyre::{eyre::eyre, Result};
use std::path::Path;
use std::process::Command;
use tracing::debug;

/// Configuration options for uploading a bootc disk image to libvirt
#[derive(Debug, Parser)]
pub struct LibvirtUploadDiskOpts {
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

    /// Skip uploading to libvirt (useful for testing)
    #[clap(long)]
    pub skip_upload: bool,

    /// Keep temporary disk image after upload
    #[clap(long)]
    pub keep_temp: bool,
}

impl LibvirtUploadDiskOpts {
    /// Generate a sanitized volume name from the container image
    fn get_volume_name(&self) -> String {
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

    /// Check if libvirt storage pool exists
    fn check_pool_exists(&self) -> Result<()> {
        let output = Command::new("virsh")
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
    fn upload_to_libvirt(&self, disk_path: &Path, disk_size_bytes: u64) -> Result<()> {
        debug!("Uploading disk to libvirt pool '{}'", self.pool);

        // Check pool exists
        self.check_pool_exists()?;

        let volume_name = self.get_volume_name();
        let volume_path = format!("{}.raw", volume_name);

        // Delete existing volume if it exists
        let _ = Command::new("virsh")
            .args(&["vol-delete", &volume_path, "--pool", &self.pool])
            .output();

        // Use the provided disk size
        let output = Command::new("virsh")
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
        let output = Command::new("virsh")
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

    /// Add metadata annotations to the libvirt volume
    fn add_volume_metadata(&self) -> Result<()> {
        let volume_name = self.get_volume_name();
        let volume_path = format!("{}.raw", volume_name);

        debug!("Adding container image metadata to volume");

        // Create XML with metadata using XmlWriter
        let mut writer = XmlWriter::new();
        writer.start_element("metadata", &[])?;
        writer.start_element(
            "bootc:container",
            &[("xmlns:bootc", "https://github.com/containers/bootc")],
        )?;
        writer.write_text_element("bootc:source-image", &self.source_image)?;
        writer.write_text_element(
            "bootc:filesystem",
            self.install.filesystem.as_deref().unwrap_or("default"),
        )?;
        writer.write_text_element("bootc:created", &chrono::Utc::now().to_rfc3339())?;
        writer.write_text_element("bootc:bcvk-version", "1.0.0")?;
        writer.end_element("bootc:container")?;
        writer.end_element("metadata")?;
        let metadata_xml = writer.into_string()?;

        // Write metadata to temp file
        let temp_metadata = std::env::temp_dir().join("volume-metadata.xml");
        std::fs::write(&temp_metadata, metadata_xml)?;

        // Set the metadata on the volume
        let _output = Command::new("virsh")
            .args(&[
                "vol-desc",
                &volume_path,
                "--pool",
                &self.pool,
                "--edit",
                "--config",
            ])
            .output()?;

        // Alternative: Use vol-dumpxml, modify, and vol-create with XML
        // This is more reliable than vol-desc which might not support metadata

        // Get current volume XML
        let output = Command::new("virsh")
            .args(&["vol-dumpxml", &volume_path, "--pool", &self.pool])
            .output()?;

        if output.status.success() {
            let xml = String::from_utf8(output.stdout)?;

            // Parse the existing volume XML using DOM parser
            let dom = xml_utils::parse_xml_dom(&xml)?;

            // Create new XML with metadata using XmlWriter
            let mut writer = XmlWriter::new();

            // Start volume element with attributes from original
            let volume_attrs = dom
                .attributes
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect::<Vec<_>>();
            writer.start_element("volume", &volume_attrs)?;

            // Copy existing elements (name, capacity, allocation, target, etc.)
            for child in &dom.children {
                if child.name != "metadata" {
                    write_xml_node(&mut writer, child)?;
                }
            }

            // Add bootc metadata
            writer.start_element("metadata", &[])?;
            writer.start_element(
                "bootc:container",
                &[("xmlns:bootc", "https://github.com/containers/bootc")],
            )?;
            writer.write_text_element("bootc:source-image", &self.source_image)?;
            writer.write_text_element(
                "bootc:filesystem",
                self.install.filesystem.as_deref().unwrap_or("default"),
            )?;
            writer.write_text_element("bootc:created", &chrono::Utc::now().to_rfc3339())?;
            writer.write_text_element("bootc:bcvk-version", "1.0.0")?;
            writer.end_element("bootc:container")?;
            writer.end_element("metadata")?;

            // Close volume element
            writer.end_element("volume")?;
            let new_xml = writer.into_string()?;

            // Save modified XML
            let temp_xml = std::env::temp_dir().join("volume-with-metadata.xml");
            std::fs::write(&temp_xml, new_xml)?;

            debug!("Added metadata to volume XML using DOM parser");
        }

        // Clean up temp file
        let _ = std::fs::remove_file(&temp_metadata);

        Ok(())
    }
}

/// Execute the libvirt disk upload process
pub fn run(opts: LibvirtUploadDiskOpts) -> Result<()> {
    debug!(
        "Starting libvirt disk upload for image: {}",
        opts.source_image
    );

    // Phase 1: Calculate disk size to use
    let disk_size = if let Some(size) = opts.disk_size {
        // Use explicit size if provided
        size.as_bytes()
    } else {
        // Use same logic as to_disk: 2x source image size with 4GB minimum
        let image_size = images::get_image_size(&opts.source_image)?;

        std::cmp::max(image_size * 2, 4u64 * 1024 * 1024 * 1024)
    };

    // Phase 2: Create temporary disk path
    let td = tempfile::TempDir::new()?;
    let td: &Utf8Path = td.path().try_into().unwrap();
    let temp_disk = td.join("disk.img");
    debug!("Using temporary disk: {temp_disk:?}");

    // Phase 3: Run installation to create disk image
    debug!("Running bootc installation to create disk image");

    let install_opts = ToDiskOpts {
        source_image: opts.source_image.clone(),
        target_disk: temp_disk.clone(),
        install: opts.install.clone(),
        additional: ToDiskAdditionalOpts {
            disk_size: Some(DiskSize::from_bytes(disk_size)),
            common: crate::run_ephemeral::CommonVmOpts {
                memory: opts.memory.clone(),
                vcpus: opts.vcpus,
                ..Default::default()
            },
            ..Default::default()
        },
    };

    to_disk(install_opts)?;

    // Phase 4: Upload to libvirt (unless skipped)
    if !opts.skip_upload {
        opts.upload_to_libvirt(temp_disk.as_std_path(), disk_size)?;
        opts.add_volume_metadata()?;

        let volume_name = opts.get_volume_name();
        debug!(
            "Successfully uploaded disk as volume '{}' to pool '{}'",
            volume_name, opts.pool
        );
        debug!("Container image annotation added: {}", opts.source_image);
    }

    // Phase 5: Cleanup temporary disk (unless keep_temp is set)
    if !opts.keep_temp && !opts.skip_upload {
        debug!("Cleaning up temporary disk");
        std::fs::remove_file(&temp_disk)?;
    } else if opts.keep_temp {
        debug!("Keeping temporary disk at: {:?}", temp_disk);
    }

    Ok(())
}

/// Helper function to recursively write an XML node and its children using XmlWriter
fn write_xml_node(writer: &mut XmlWriter, node: &xml_utils::XmlNode) -> Result<()> {
    let attrs = node
        .attributes
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect::<Vec<_>>();

    if node.children.is_empty() && node.text.is_empty() {
        // Empty element
        writer.write_empty_element(&node.name, &attrs)?;
    } else if node.children.is_empty() {
        // Simple text element
        writer.write_text_element_with_attrs(&node.name, &node.text, &attrs)?;
    } else {
        // Element with children
        writer.start_element(&node.name, &attrs)?;
        if !node.text.is_empty() {
            writer.write_text(&node.text)?;
        }
        for child in &node.children {
            write_xml_node(writer, child)?;
        }
        writer.end_element(&node.name)?;
    }

    Ok(())
}
