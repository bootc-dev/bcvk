//! systemd-repart integration for declarative disk image creation.
//!
//! This module provides a Rust interface to systemd-repart, which creates
//! and manages disk images using declarative configuration files.
//!
//! systemd-repart creates GPT-partitioned disk images with formatted filesystems
//! and can populate them with content. This is useful for creating bootable images,
//! data disks, and other disk-based artifacts.

#![allow(dead_code)]

use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::eyre::{Context as _, eyre};
use color_eyre::Result;
use std::fs;
use std::process::Command;
use tracing::debug;

/// Configuration for a partition to be created by systemd-repart.
#[derive(Debug, Clone)]
pub struct PartitionConfig {
    /// Partition type (e.g., "esp", "linux-generic", "home", "srv", "swap")
    pub partition_type: String,
    /// Filesystem format (e.g., "vfat", "ext4", "btrfs", "xfs")
    pub format: Option<String>,
    /// Filesystem label
    pub label: Option<String>,
    /// Minimum size in bytes
    pub size_min_bytes: Option<u64>,
    /// Maximum size in bytes
    pub size_max_bytes: Option<u64>,
    /// Source directory to copy files from (will be copied to root of partition)
    pub copy_files_source: Option<Utf8PathBuf>,
}

impl PartitionConfig {
    /// Create a new partition configuration.
    pub fn new(partition_type: impl Into<String>) -> Self {
        Self {
            partition_type: partition_type.into(),
            format: None,
            label: None,
            size_min_bytes: None,
            size_max_bytes: None,
            copy_files_source: None,
        }
    }

    /// Set the filesystem format.
    pub fn with_format(mut self, format: impl Into<String>) -> Self {
        self.format = Some(format.into());
        self
    }

    /// Set the filesystem label.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the minimum partition size in megabytes.
    pub fn with_size_min_mb(mut self, mb: u64) -> Self {
        self.size_min_bytes = Some(mb * 1024 * 1024);
        self
    }

    /// Set the maximum partition size in megabytes.
    pub fn with_size_max_mb(mut self, mb: u64) -> Self {
        self.size_max_bytes = Some(mb * 1024 * 1024);
        self
    }

    /// Set both min and max size to the same value (fixed size partition).
    pub fn with_size_mb(mut self, mb: u64) -> Self {
        let bytes = mb * 1024 * 1024;
        self.size_min_bytes = Some(bytes);
        self.size_max_bytes = Some(bytes);
        self
    }

    /// Copy files from a source directory into the partition.
    pub fn with_copy_files(mut self, source: impl Into<Utf8PathBuf>) -> Self {
        self.copy_files_source = Some(source.into());
        self
    }

    /// Generate the repart.d configuration file content for this partition.
    fn to_repart_conf(&self) -> String {
        let mut conf = String::new();
        conf.push_str("[Partition]\n");
        conf.push_str(&format!("Type={}\n", self.partition_type));

        if let Some(ref format) = self.format {
            conf.push_str(&format!("Format={}\n", format));
        }

        if let Some(ref label) = self.label {
            conf.push_str(&format!("Label={}\n", label));
        }

        if let Some(size) = self.size_min_bytes {
            conf.push_str(&format!("SizeMinBytes={}\n", size));
        }

        if let Some(size) = self.size_max_bytes {
            conf.push_str(&format!("SizeMaxBytes={}\n", size));
        }

        if let Some(ref source) = self.copy_files_source {
            conf.push_str(&format!("CopyFiles={}:/\n", source));
        }

        conf
    }
}

/// Builder for creating disk images using systemd-repart.
#[derive(Debug)]
pub struct RepartImageBuilder {
    partitions: Vec<PartitionConfig>,
    size_auto: bool,
}

impl RepartImageBuilder {
    /// Create a new image builder.
    pub fn new() -> Self {
        Self {
            partitions: Vec::new(),
            size_auto: true,
        }
    }

    /// Add a partition to the image.
    pub fn add_partition(mut self, partition: PartitionConfig) -> Self {
        self.partitions.push(partition);
        self
    }

    /// Generate a disk image at the specified path.
    ///
    /// Creates a GPT-partitioned disk image with the configured partitions.
    /// Each partition is formatted with its specified filesystem and populated
    /// with any configured content.
    ///
    /// # Arguments
    ///
    /// * `output_path` - Path where the disk image will be created
    ///
    /// # Returns
    ///
    /// Returns the path to the created disk image.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - systemd-repart is not available
    /// - Configuration files cannot be written
    /// - systemd-repart execution fails
    pub fn generate(&self, output_path: impl AsRef<Utf8Path>) -> Result<Utf8PathBuf> {
        let output_path = output_path.as_ref();

        // Check if systemd-repart is available
        if which::which("systemd-repart").is_err() {
            return Err(eyre!(
                "systemd-repart not found. Please install systemd package:\n\
                 - Fedora/RHEL: sudo dnf install systemd\n\
                 - Debian/Ubuntu: sudo apt install systemd"
            ));
        }

        if self.partitions.is_empty() {
            return Err(eyre!("No partitions configured for disk image"));
        }

        // Create temporary directory for repart.d configuration
        let temp_dir = tempfile::tempdir()
            .context("Failed to create temporary directory for repart.d configuration")?;
        let temp_path = Utf8PathBuf::try_from(temp_dir.path().to_path_buf())
            .context("Invalid UTF-8 in temp directory path")?;

        let repart_d_dir = temp_path.join("repart.d");
        fs::create_dir_all(&repart_d_dir)
            .with_context(|| format!("Failed to create repart.d directory at {}", repart_d_dir))?;

        debug!(
            "Creating systemd-repart configuration in: {}",
            repart_d_dir
        );

        // Write partition configuration files
        for (i, partition) in self.partitions.iter().enumerate() {
            let conf_filename = format!("{:02}-partition.conf", i * 10);
            let conf_path = repart_d_dir.join(conf_filename);

            let conf_content = partition.to_repart_conf();
            fs::write(&conf_path, conf_content).with_context(|| {
                format!("Failed to write repart configuration to {}", conf_path)
            })?;

            debug!("Wrote repart configuration: {}", conf_path);
        }

        // Run systemd-repart to create the image
        debug!("Running systemd-repart to create disk image: {}", output_path);

        let mut cmd = Command::new("systemd-repart");
        cmd.arg("--definitions")
            .arg(repart_d_dir.as_str())
            .arg("--empty=create")
            .arg("--dry-run=no");

        if self.size_auto {
            cmd.arg("--size=auto");
        }

        cmd.arg(output_path.as_str());

        let output = cmd
            .output()
            .context("Failed to execute systemd-repart")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(eyre!(
                "systemd-repart failed (exit code: {}):\nstdout: {}\nstderr: {}",
                output.status.code().unwrap_or(-1),
                stdout,
                stderr
            ));
        }

        debug!("Disk image created successfully at: {}", output_path);

        Ok(output_path.to_owned())
    }
}

impl Default for RepartImageBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a simple VFAT disk image with content using systemd-repart.
///
/// This is a convenience function that creates a GPT-partitioned disk image
/// with a single VFAT partition containing the specified content.
///
/// Note: This creates a GPT-partitioned disk, not a raw VFAT filesystem.
/// For raw VFAT filesystems (e.g., for cloud-init ConfigDrive), use
/// the mkfs.vfat-based approach in the cloud_init module instead.
///
/// # Arguments
///
/// * `source_dir` - Directory whose contents will be copied to the VFAT partition
/// * `label` - Filesystem label for the VFAT partition
/// * `output_path` - Path where the disk image will be created
///
/// # Returns
///
/// Returns the path to the created disk image.
///
/// # Example
///
/// ```no_run
/// use camino::Utf8PathBuf;
/// # fn example() -> color_eyre::Result<()> {
/// let source = Utf8PathBuf::from("/tmp/data");
/// let output = Utf8PathBuf::from("/tmp/data.img");
/// bcvk::repart::create_vfat_image(&source, "MY-DATA", &output)?;
/// # Ok(())
/// # }
/// ```
pub fn create_vfat_image(
    source_dir: impl AsRef<Utf8Path>,
    label: impl Into<String>,
    output_path: impl AsRef<Utf8Path>,
) -> Result<Utf8PathBuf> {
    let partition = PartitionConfig::new("esp")
        .with_format("vfat")
        .with_label(label)
        .with_size_mb(10)
        .with_copy_files(source_dir.as_ref().to_owned());

    RepartImageBuilder::new()
        .add_partition(partition)
        .generate(output_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_partition_config_builder() {
        let config = PartitionConfig::new("esp")
            .with_format("vfat")
            .with_label("TEST")
            .with_size_mb(10);

        assert_eq!(config.partition_type, "esp");
        assert_eq!(config.format, Some("vfat".to_string()));
        assert_eq!(config.label, Some("TEST".to_string()));
        assert_eq!(config.size_min_bytes, Some(10 * 1024 * 1024));
        assert_eq!(config.size_max_bytes, Some(10 * 1024 * 1024));
    }

    #[test]
    fn test_partition_config_to_repart_conf() {
        let config = PartitionConfig::new("esp")
            .with_format("vfat")
            .with_label("TEST")
            .with_size_mb(10);

        let conf = config.to_repart_conf();
        assert!(conf.contains("Type=esp"));
        assert!(conf.contains("Format=vfat"));
        assert!(conf.contains("Label=TEST"));
        assert!(conf.contains("SizeMinBytes="));
        assert!(conf.contains("SizeMaxBytes="));
    }

    #[test]
    fn test_builder_no_partitions() {
        let builder = RepartImageBuilder::new();
        let temp_dir = tempfile::tempdir().unwrap();
        let output = Utf8PathBuf::try_from(temp_dir.path().join("test.img")).unwrap();

        let result = builder.generate(&output);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No partitions configured"));
    }
}
