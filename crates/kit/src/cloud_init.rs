//! Cloud-init ConfigDrive generation for VM configuration.
//!
//! Creates cloud-init ConfigDrive VFAT filesystems that can be attached to VMs to provide
//! initial configuration. The ConfigDrive follows the OpenStack ConfigDrive v2 format.
//!
//! This implementation uses the same approach as systemd-repart for populating VFAT filesystems:
//! - `mkfs.vfat` to create the VFAT filesystem
//! - `mcopy` (from mtools) to populate files into the VFAT image

use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::eyre::{eyre, Context as _};
use color_eyre::Result;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::process::Command;
use tracing::debug;

/// Volume label used by cloud-init for ConfigDrive datasource
/// Using uppercase as cloud-init documentation specifies "CONFIG-2" or "config-2"
const CONFIG_DRIVE_LABEL: &str = "CONFIG-2";

/// Cloud-init configuration for generating ConfigDrive images.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudInitConfig {
    /// Hostname to set in the guest
    pub hostname: Option<String>,
    /// Custom user-data YAML content (will be merged with generated config)
    pub user_data_yaml: Option<String>,
}

impl CloudInitConfig {
    /// Create a new cloud-init configuration builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Set custom user-data YAML content
    pub fn with_user_data(mut self, user_data: String) -> Self {
        self.user_data_yaml = Some(user_data);
        self
    }

    /// Generate a VFAT filesystem with ConfigDrive format at the specified path.
    ///
    /// Creates a VFAT filesystem image containing cloud-init data in the ConfigDrive
    /// format expected by OpenStack and other cloud environments. The filesystem has
    /// the label "config-2" and contains the directory structure:
    ///
    /// ```text
    /// openstack/
    ///   latest/
    ///     meta_data.json
    ///     user_data
    /// ```
    ///
    /// This uses `mkfs.vfat` which is more commonly available than `genisoimage`.
    pub fn generate_vfat_configdrive(&self, output_path: impl AsRef<Utf8Path>) -> Result<()> {
        let output_path = output_path.as_ref();

        // Create temporary directory for ConfigDrive structure
        let temp_dir = tempfile::tempdir()
            .context("Failed to create temporary directory for ConfigDrive files")?;
        let temp_path = Utf8PathBuf::try_from(temp_dir.path().to_path_buf())
            .context("Invalid UTF-8 in temp directory path")?;

        debug!(
            "Creating ConfigDrive structure in temporary directory: {}",
            temp_path
        );

        // Create openstack/latest directory structure
        let openstack_dir = temp_path.join("openstack").join("latest");
        fs::create_dir_all(&openstack_dir).with_context(|| {
            format!(
                "Failed to create openstack/latest directory at {}",
                openstack_dir
            )
        })?;

        // Write meta_data.json
        self.write_configdrive_metadata(&openstack_dir)?;

        // Write user_data
        self.write_configdrive_userdata(&openstack_dir)?;

        // Create VFAT filesystem image
        self.create_vfat_image(&temp_path, output_path)?;

        debug!(
            "ConfigDrive VFAT image created successfully at: {}",
            output_path
        );

        Ok(())
    }

    /// Write meta_data.json for ConfigDrive format
    fn write_configdrive_metadata(&self, openstack_dir: &Utf8Path) -> Result<()> {
        let meta_data_path = openstack_dir.join("meta_data.json");

        // Create metadata JSON structure following OpenStack schema
        // IMPORTANT: cloud-init expects specific field names (see KEY_COPIES in
        // cloudinit/sources/helpers/openstack.py):
        // - "uuid" (required) -> cloud-init copies to "instance-id"
        // - "hostname" (optional) -> cloud-init copies to "local-hostname"
        let mut meta_obj = serde_json::Map::new();
        meta_obj.insert(
            "uuid".to_string(),
            serde_json::Value::String("iid-local01".to_string()),
        );

        if let Some(ref hostname) = self.hostname {
            meta_obj.insert(
                "hostname".to_string(),
                serde_json::Value::String(hostname.clone()),
            );
        }

        let meta_json = serde_json::to_string_pretty(&meta_obj)
            .context("Failed to serialize meta_data.json")?;

        fs::write(&meta_data_path, meta_json)
            .with_context(|| format!("Failed to write meta_data.json to {}", meta_data_path))?;

        debug!("Wrote meta_data.json file: {}", meta_data_path);
        Ok(())
    }

    /// Write user_data for ConfigDrive format
    fn write_configdrive_userdata(&self, openstack_dir: &Utf8Path) -> Result<()> {
        let user_data_path = openstack_dir.join("user_data");

        let mut user_data = if let Some(ref custom_yaml) = self.user_data_yaml {
            // Parse existing YAML and merge
            serde_yaml::from_str::<serde_yaml::Value>(custom_yaml)
                .context("Failed to parse custom user-data YAML")?
        } else {
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
        };

        // Ensure we have a mapping
        let user_data_map = user_data
            .as_mapping_mut()
            .ok_or_else(|| eyre!("user-data must be a YAML mapping/object"))?;

        // Ensure we don't have conflicting SSH key configuration
        if user_data_map.contains_key("ssh_authorized_keys") {
            debug!("User provided ssh_authorized_keys in custom YAML");
        }

        // Write user_data file with #cloud-config shebang
        let mut file = File::create(&user_data_path)
            .with_context(|| format!("Failed to create user_data file at {}", user_data_path))?;

        // Write shebang
        file.write_all(b"#cloud-config\n")
            .context("Failed to write cloud-config shebang")?;

        // Write YAML content
        serde_yaml::to_writer(&file, &user_data)
            .context("Failed to write user_data YAML content")?;

        debug!("Wrote user_data file: {}", user_data_path);
        Ok(())
    }

    /// Create VFAT filesystem image using mkfs.vfat and mtools
    fn create_vfat_image(&self, source_dir: &Utf8Path, output_path: &Utf8Path) -> Result<()> {
        // Check if mkfs.vfat is available
        if which::which("mkfs.vfat").is_err() {
            return Err(eyre!(
                "mkfs.vfat not found. Please install dosfstools package:\n\
                 - Fedora/RHEL: sudo dnf install dosfstools\n\
                 - Debian/Ubuntu: sudo apt install dosfstools"
            ));
        }

        // Check if mtools (mcopy, mmd) is available
        if which::which("mcopy").is_err() {
            return Err(eyre!(
                "mcopy not found. Please install mtools package:\n\
                 - Fedora/RHEL: sudo dnf install mtools\n\
                 - Debian/Ubuntu: sudo apt install mtools"
            ));
        }

        // Create a 10MB VFAT image (sufficient for cloud-init data)
        let image_size_mb = 10;
        debug!(
            "Creating {}MB VFAT image at: {}",
            image_size_mb, output_path
        );

        // Create sparse file
        let create_status = Command::new("dd")
            .args([
                "if=/dev/zero",
                &format!("of={}", output_path),
                "bs=1M",
                &format!("count={}", image_size_mb),
                "status=none",
            ])
            .status()
            .context("Failed to execute dd command to create image file")?;

        if !create_status.success() {
            return Err(eyre!(
                "Failed to create image file with dd (exit code: {})",
                create_status.code().unwrap_or(-1)
            ));
        }

        // Format as VFAT with the config-2 label
        let mkfs_output = Command::new("mkfs.vfat")
            .args(["-n", CONFIG_DRIVE_LABEL, output_path.as_str()])
            .output()
            .context("Failed to execute mkfs.vfat")?;

        if !mkfs_output.status.success() {
            let stderr = String::from_utf8_lossy(&mkfs_output.stderr);
            return Err(eyre!(
                "Failed to format VFAT filesystem (exit code: {}): {}",
                mkfs_output.status.code().unwrap_or(-1),
                stderr
            ));
        }

        debug!(
            "Formatted VFAT filesystem with label: {}",
            CONFIG_DRIVE_LABEL
        );

        // Use mtools to copy files into the VFAT image without mounting
        // First, create the openstack/latest directory structure
        let mmd_output = Command::new("mmd")
            .args([
                "-i",
                output_path.as_str(),
                "::openstack",
                "::openstack/latest",
            ])
            .output()
            .context("Failed to execute mmd command")?;

        if !mmd_output.status.success() {
            let stderr = String::from_utf8_lossy(&mmd_output.stderr);
            return Err(eyre!(
                "Failed to create openstack directory in VFAT image: {}",
                stderr
            ));
        }

        debug!("Created openstack/latest directory structure in VFAT image");

        // Copy meta_data.json
        let meta_data_src = source_dir.join("openstack/latest/meta_data.json");
        let mcopy_meta_output = Command::new("mcopy")
            .args([
                "-i",
                output_path.as_str(),
                meta_data_src.as_str(),
                "::openstack/latest/meta_data.json",
            ])
            .output()
            .context("Failed to execute mcopy for meta_data.json")?;

        if !mcopy_meta_output.status.success() {
            let stderr = String::from_utf8_lossy(&mcopy_meta_output.stderr);
            return Err(eyre!(
                "Failed to copy meta_data.json to VFAT image: {}",
                stderr
            ));
        }

        debug!("Copied meta_data.json to VFAT image");

        // Copy user_data
        let user_data_src = source_dir.join("openstack/latest/user_data");
        let mcopy_user_output = Command::new("mcopy")
            .args([
                "-i",
                output_path.as_str(),
                user_data_src.as_str(),
                "::openstack/latest/user_data",
            ])
            .output()
            .context("Failed to execute mcopy for user_data")?;

        if !mcopy_user_output.status.success() {
            let stderr = String::from_utf8_lossy(&mcopy_user_output.stderr);
            return Err(eyre!("Failed to copy user_data to VFAT image: {}", stderr));
        }

        debug!("Copied user_data to VFAT image");

        Ok(())
    }
}

/// Generate a ConfigDrive VFAT filesystem from a user-provided cloud-config file.
///
/// This is a convenience function that creates a ConfigDrive with the contents of the
/// specified cloud-config file.
///
/// # Arguments
///
/// * `user_data_path` - Path to a cloud-config YAML file
/// * `output_path` - Path where the ConfigDrive image will be created
///
/// # Returns
///
/// Returns the path to the created ConfigDrive image.
///
/// # Errors
///
/// Returns an error if:
/// - The user-data file cannot be read
/// - The user-data file is not valid YAML
/// - Required tools (mkfs.vfat, mtools) are not available
/// - ConfigDrive generation fails
pub fn generate_configdrive_from_file(
    user_data_path: impl AsRef<Utf8Path>,
    output_path: impl AsRef<Utf8Path>,
) -> Result<Utf8PathBuf> {
    let user_data_path = user_data_path.as_ref();
    let output_path = output_path.as_ref();

    debug!(
        "Generating ConfigDrive from user-data file: {}",
        user_data_path
    );

    // Read the user-provided cloud-config file
    let user_data_content = fs::read_to_string(user_data_path).with_context(|| {
        format!(
            "Failed to read cloud-config file at {}. Please ensure the file exists and is readable.",
            user_data_path
        )
    })?;

    // Validate it's valid YAML
    let _: serde_yaml::Value = serde_yaml::from_str(&user_data_content).with_context(|| {
        format!(
            "Failed to parse cloud-config file at {} as YAML. Please ensure it's valid YAML format.",
            user_data_path
        )
    })?;

    // Create CloudInitConfig with the user-provided data
    let config = CloudInitConfig::new().with_user_data(user_data_content);

    // Generate the VFAT ConfigDrive
    config.generate_vfat_configdrive(output_path)?;

    debug!("ConfigDrive generated at: {}", output_path);

    Ok(output_path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cloud_init_config_builder() {
        let config = CloudInitConfig::new();
        assert_eq!(config.hostname, None);
        assert_eq!(config.user_data_yaml, None);
    }

    #[test]
    fn test_meta_data_generation() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let temp_path = Utf8PathBuf::try_from(temp_dir.path().to_path_buf())?;

        let config = CloudInitConfig::new();
        config.write_configdrive_metadata(&temp_path)?;

        let meta_data_path = temp_path.join("meta_data.json");
        assert!(meta_data_path.exists());

        let content = fs::read_to_string(&meta_data_path)?;
        assert!(content.contains("uuid"));
        assert!(content.contains("iid-local01"));

        Ok(())
    }

    #[test]
    fn test_user_data_generation() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let temp_path = Utf8PathBuf::try_from(temp_dir.path().to_path_buf())?;

        let custom_yaml = r#"
runcmd:
  - echo "Hello World"
"#;

        let config = CloudInitConfig::new().with_user_data(custom_yaml.to_string());
        config.write_configdrive_userdata(&temp_path)?;

        let user_data_path = temp_path.join("user_data");
        assert!(user_data_path.exists());

        let content = fs::read_to_string(&user_data_path)?;
        assert!(content.starts_with("#cloud-config"));
        assert!(content.contains("Hello World"));

        Ok(())
    }
}
