//! Project configuration file parsing for `.bcvk/config.toml`

use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::{eyre::Context as _, Result};
use serde::{Deserialize, Serialize};
use std::fs;

/// Configuration file name within project directory
pub const CONFIG_DIR: &str = ".bcvk";
pub const CONFIG_FILE: &str = "config.toml";

/// Project configuration loaded from `.bcvk/config.toml`
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ProjectConfig {
    /// Project metadata
    pub project: Option<ProjectInfo>,

    /// VM configuration
    pub vm: Option<VmConfig>,

    /// Volume mounts
    pub mounts: Option<Vec<MountConfig>>,

    /// Systemd configuration
    pub systemd: Option<SystemdConfig>,
}

/// Project metadata section
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ProjectInfo {
    /// Optional project name override (defaults to directory name)
    pub name: Option<String>,
}

/// VM configuration section
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct VmConfig {
    /// Container image to run as bootable VM
    #[serde(default)]
    pub image: String,

    /// Memory allocation (e.g., "4G", "2048M")
    #[serde(flatten, default)]
    pub memory: crate::common_opts::MemoryOpts,

    /// Number of virtual CPUs
    #[serde(flatten, default)]
    pub cpu: crate::common_opts::CpuOpts,

    /// Disk size (e.g., "20G", "50G")
    #[serde(flatten, default)]
    pub disk: crate::common_opts::DiskSizeOpts,

    /// Network mode
    #[serde(flatten, default)]
    pub net: crate::common_opts::NetworkOpts,

    /// Root filesystem type
    pub filesystem: Option<String>,
}

/// Mount configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct MountConfig {
    /// Host path to mount
    pub host: String,

    /// Guest mount point or tag
    pub guest: String,

    /// Mount as read-only
    #[serde(default)]
    pub readonly: bool,
}

/// Systemd configuration section
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SystemdConfig {
    /// Directory containing systemd units (relative to project root)
    pub units_dir: Option<String>,
}

impl ProjectConfig {
    /// Load project configuration from directory
    ///
    /// Looks for `.bcvk/config.toml` in the given directory.
    /// Returns `None` if the config file doesn't exist.
    pub fn load_from_dir(dir: &Utf8Path) -> Result<Option<Self>> {
        let config_path = dir.join(CONFIG_DIR).join(CONFIG_FILE);

        if !config_path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path))?;

        let config: ProjectConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", config_path))?;

        // Validate that vm section and image are specified
        if let Some(ref vm) = config.vm {
            if vm.image.is_empty() {
                color_eyre::eyre::bail!(
                    "vm.image must be specified in config file: {}",
                    config_path
                );
            }
        } else {
            color_eyre::eyre::bail!(
                "vm section must be specified in config file: {}",
                config_path
            );
        }

        Ok(Some(config))
    }

    /// Get the systemd units directory path
    ///
    /// Returns the absolute path to the systemd units directory if configured.
    pub fn systemd_units_dir(&self, project_dir: &Utf8Path) -> Option<Utf8PathBuf> {
        self.systemd
            .as_ref()
            .and_then(|systemd| systemd.units_dir.as_ref())
            .map(|dir| project_dir.join(dir))
    }

    /// Get the .bcvk/units directory path if it exists
    ///
    /// This is the default location for systemd units.
    pub fn default_units_dir(project_dir: &Utf8Path) -> Option<Utf8PathBuf> {
        let units_dir = project_dir.join(CONFIG_DIR).join("units");
        if units_dir.exists() {
            Some(units_dir)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_config() {
        let toml = r#"
[vm]
image = "localhost/my-image"
"#;

        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let vm = config.vm.as_ref().unwrap();
        assert_eq!(vm.image, "localhost/my-image");
        assert_eq!(vm.memory.memory, "4G");
        assert_eq!(vm.cpu.cpus, 2);
        assert_eq!(vm.disk.disk_size, "20G");
    }

    #[test]
    fn test_parse_full_config() {
        let toml = r#"
[project]
name = "my-project"

[vm]
image = "localhost/my-image"
memory = "8G"
cpus = 4
disk-size = "50G"
network = "bridge=br0"
filesystem = "xfs"

[[mounts]]
host = "/data"
guest = "/mnt/data"
readonly = true

[[mounts]]
host = "/tmp/workspace"
guest = "/workspace"
readonly = false

[systemd]
units-dir = "systemd-units"
"#;

        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let project = config.project.as_ref().unwrap();
        assert_eq!(project.name, Some("my-project".to_string()));
        let vm = config.vm.as_ref().unwrap();
        assert_eq!(vm.image, "localhost/my-image");
        assert_eq!(vm.memory.memory, "8G");
        assert_eq!(vm.cpu.cpus, 4);
        assert_eq!(vm.disk.disk_size, "50G");
        assert_eq!(vm.net.network, "bridge=br0");
        assert_eq!(vm.filesystem, Some("xfs".to_string()));
        assert_eq!(config.mounts.as_ref().unwrap().len(), 2);
        assert_eq!(config.mounts.as_ref().unwrap()[0].host, "/data");
        assert_eq!(config.mounts.as_ref().unwrap()[0].readonly, true);
        let systemd = config.systemd.as_ref().unwrap();
        assert_eq!(systemd.units_dir, Some("systemd-units".to_string()));
    }
}
