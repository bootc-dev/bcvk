//! Secure Boot key management for libvirt/QEMU virtualization
//!
//! This module provides utilities for loading existing UEFI Secure Boot
//! keys (PK, KEK, db) and customizing OVMF firmware variables for VMs.

use camino::{Utf8Path, Utf8PathBuf};
use cap_std_ext::cap_std::fs::Dir;
use cap_std_ext::dirext::{CapStdExtDirExt, WalkConfiguration};
use color_eyre::{eyre::eyre, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::ops::ControlFlow;

/// System-wide QEMU firmware descriptor search directories
const QEMU_FIRMWARE_DIRS: &[&str] = &["/etc/qemu/firmware", "/usr/share/qemu/firmware"];

/// QEMU firmware descriptor executable section
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct FirmwareExecutable {
    /// Path to the firmware file
    pub(crate) filename: String,
    /// Format of the firmware file (e.g., "raw")
    pub(crate) format: String,
}

/// QEMU firmware descriptor nvram template section
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct FirmwareNvramTemplate {
    /// Path to the NVRAM template file
    pub(crate) filename: String,
    /// Format of the NVRAM template file (e.g., "raw")
    pub(crate) format: String,
}

/// QEMU firmware descriptor mapping section
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct FirmwareMapping {
    /// Device type
    pub(crate) device: String,
    /// Executable (firmware) configuration (for flash device type)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) executable: Option<FirmwareExecutable>,
    /// NVRAM template configuration (for flash device type)
    #[serde(rename = "nvram-template", skip_serializing_if = "Option::is_none")]
    pub(crate) nvram_template: Option<FirmwareNvramTemplate>,
    /// Single firmware filename (for memory device type)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) filename: Option<String>,
}

impl FirmwareMapping {
    /// QEMU firmware device type: memory-mapped
    pub(crate) const DEVICE_TYPE_MEMORY: &'static str = "memory";

    /// QEMU firmware device type: flash
    #[allow(dead_code)]
    pub(crate) const DEVICE_TYPE_FLASH: &'static str = "flash";
}

/// QEMU firmware descriptor target architecture
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct FirmwareTarget {
    /// Architecture name (e.g., "x86_64", "aarch64")
    pub(crate) architecture: String,
    /// Supported machines
    pub(crate) machines: Vec<String>,
}

/// QEMU firmware descriptor (QEMU firmware interop specification)
/// See: https://qemu.readthedocs.io/en/latest/interop/firmware.json.html
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct FirmwareDescriptor {
    /// Human-readable description
    pub(crate) description: String,
    /// Interface types (e.g., "uefi")
    #[serde(rename = "interface-types")]
    pub(crate) interface_types: Vec<String>,
    /// Firmware mapping
    pub(crate) mapping: FirmwareMapping,
    /// Target architectures
    pub(crate) targets: Vec<FirmwareTarget>,
    /// Features (e.g., "secure-boot", "enrolled-keys")
    pub(crate) features: Vec<String>,
    /// Tags
    pub(crate) tags: Vec<String>,
}

impl FirmwareDescriptor {
    /// QEMU firmware feature: secure boot support
    pub(crate) const FEATURE_SECURE_BOOT: &'static str = "secure-boot";

    /// QEMU firmware feature: enrolled keys
    pub(crate) const FEATURE_ENROLLED_KEYS: &'static str = "enrolled-keys";

    /// Check if this firmware supports secure boot
    pub(crate) fn supports_secure_boot(&self) -> bool {
        self.features
            .contains(&Self::FEATURE_SECURE_BOOT.to_string())
    }

    /// Check if this firmware has enrolled keys
    pub(crate) fn has_enrolled_keys(&self) -> bool {
        self.features
            .contains(&Self::FEATURE_ENROLLED_KEYS.to_string())
    }

    /// Check if this firmware supports the given architecture
    pub(crate) fn supports_architecture(&self, arch: &str) -> bool {
        self.targets.iter().any(|t| t.architecture == arch)
    }
}

/// UEFI firmware paths and formats from QEMU firmware interop descriptors
#[derive(Debug, Clone)]
pub struct FirmwareInfo {
    /// Path to OVMF_CODE firmware file
    pub code_path: Utf8PathBuf,
    /// Format of the OVMF_CODE file (raw, qcow2)
    pub code_format: String,
    /// Path to OVMF_VARS template file
    pub vars_path: Utf8PathBuf,
    /// Format of the OVMF_VARS file (raw, qcow2)
    pub vars_format: String,
}

/// Secure Boot key configuration
#[derive(Debug, Clone)]
pub struct SecureBootConfig {
    /// Directory containing the secure boot keys
    pub key_dir: Utf8PathBuf,
    /// Path to custom OVMF_VARS template with enrolled keys
    pub vars_template: Utf8PathBuf,
    /// Format of the NVRAM template file (raw, qcow2)
    pub vars_format: String,
    /// GUID for the key owner
    #[allow(dead_code)]
    pub guid: String,
}

/// Secure Boot key set
#[derive(Debug)]
pub struct SecureBootKeys {
    /// Platform Key certificate
    pub pk_cert: Utf8PathBuf,
    /// Key Exchange Key certificate
    pub kek_cert: Utf8PathBuf,
    /// Signature Database certificate
    pub db_cert: Utf8PathBuf,
    /// Owner GUID
    pub guid: String,
}

impl SecureBootKeys {
    /// Load existing secure boot keys from a directory
    pub fn load(key_dir: &Utf8Path) -> Result<Self> {
        // Check if directory exists
        let mut test_dir = std::process::Command::new("test");
        test_dir.args(["-d", key_dir.as_str()]);

        if !test_dir.status()?.success() {
            return Err(eyre!(
                "Secure boot key directory not found: {}. Please generate keys externally.",
                key_dir
            ));
        }

        let guid_file = key_dir.join("GUID.txt");

        // Read GUID file
        let mut cat_guid = std::process::Command::new("cat");
        cat_guid.arg(guid_file.as_str());
        let guid_output = cat_guid.output()?;

        if !guid_output.status.success() {
            return Err(eyre!(
                "Failed to read GUID from {}. Ensure keys are properly generated.",
                guid_file
            ));
        }

        let guid = String::from_utf8(guid_output.stdout)
            .map_err(|_| eyre!("Invalid UTF-8 in GUID file"))?
            .trim()
            .to_string();

        let keys = Self {
            pk_cert: key_dir.join("PK.crt"),
            kek_cert: key_dir.join("KEK.crt"),
            db_cert: key_dir.join("db.crt"),
            guid,
        };

        // Verify all required files exist
        let required_files = [
            (&keys.pk_cert, "PK.crt"),
            (&keys.kek_cert, "KEK.crt"),
            (&keys.db_cert, "db.crt"),
        ];

        for (path, name) in &required_files {
            let mut test_file = std::process::Command::new("test");
            test_file.args(["-f", path.as_str()]);

            if !test_file.status()?.success() {
                return Err(eyre!(
                    "Required secure boot file {} not found in {}",
                    name,
                    key_dir
                ));
            }
        }

        Ok(keys)
    }
}

/// Customize OVMF variables with secure boot keys
pub fn customize_ovmf_vars(
    keys: &SecureBootKeys,
    ovmf_vars_path: &Utf8Path,
    output_path: &Utf8Path,
) -> Result<()> {
    // Check if virt-fw-vars is available
    let mut check = std::process::Command::new("which");
    check.arg("virt-fw-vars");
    let check_output = check.output()?;

    if !check_output.status.success() {
        return Err(eyre!("virt-fw-vars tool not found"));
    }

    // Use virt-fw-vars to inject keys into OVMF_VARS
    let mut cmd = std::process::Command::new("virt-fw-vars");
    cmd.args([
        "--input",
        ovmf_vars_path.as_str(),
        "--secure-boot",
        "--set-pk",
        &keys.guid,
        keys.pk_cert.as_str(),
        "--add-kek",
        &keys.guid,
        keys.kek_cert.as_str(),
        "--add-db",
        &keys.guid,
        keys.db_cert.as_str(),
        "-o",
        output_path.as_str(),
    ]);

    let output = cmd.output()?;

    if !output.status.success() {
        return Err(eyre!(
            "Failed to customize OVMF variables: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}

/// File name of the per-domain OVMF_VARS template with enrolled keys, which
/// is kept in the libvirt storage pool.
pub(crate) fn vars_template_filename(domain_name: &str) -> String {
    format!("{domain_name}_OVMF_VARS.fd")
}

/// Returns the per-domain VARS template bcvk created for `domain_name`, if its
/// domain XML uses one. A system template (e.g. from edk2) is never returned.
pub(crate) fn owned_vars_template(
    dom: &crate::xml_utils::XmlNode,
    domain_name: &str,
) -> Option<Utf8PathBuf> {
    let template = Utf8PathBuf::from(dom.find("nvram")?.attributes.get("template")?);
    let expected = vars_template_filename(domain_name);
    (template.file_name() == Some(expected.as_str())).then_some(template)
}

/// Load and setup secure boot configuration from existing keys
///
/// The `vars_output_path` should be in the libvirt storage pool. It is
/// always regenerated: a file left behind by an earlier VM with the same
/// name may have different keys enrolled.
pub fn setup_secure_boot(
    key_dir: &Utf8Path,
    vars_output_path: &Utf8Path,
) -> Result<SecureBootConfig> {
    tracing::info!("Loading secure boot keys from {}", key_dir);
    let keys = SecureBootKeys::load(key_dir)?;

    // Find the system firmware (includes format info)
    let firmware_info = find_firmware_from_descriptors(true)?;

    tracing::info!(
        "Creating custom OVMF_VARS template with enrolled keys at {}",
        vars_output_path
    );
    customize_ovmf_vars(&keys, &firmware_info.vars_path, vars_output_path)?;

    // virt-fw-vars preserves the input format, so the output has the same format as the input
    Ok(SecureBootConfig {
        key_dir: key_dir.to_owned(),
        vars_template: vars_output_path.to_owned(),
        vars_format: firmware_info.vars_format,
        guid: keys.guid,
    })
}

/// Get firmware search directories following QEMU firmware interop specification
fn get_firmware_search_dirs() -> Vec<Utf8PathBuf> {
    let mut dirs = Vec::new();

    // $XDG_CONFIG_HOME/qemu/firmware or $HOME/.config/qemu/firmware
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        if let Ok(mut path) = Utf8PathBuf::try_from(config_home) {
            path.push("qemu/firmware");
            dirs.push(path);
        }
    } else if let Some(home) = std::env::var_os("HOME") {
        if let Ok(mut path) = Utf8PathBuf::try_from(home) {
            path.push(".config/qemu/firmware");
            dirs.push(path);
        }
    }

    // System-wide directories
    dirs.extend(QEMU_FIRMWARE_DIRS.iter().map(|d| Utf8PathBuf::from(d)));

    dirs
}

/// List all firmware descriptor JSON files
pub(crate) fn list_firmware_descriptors() -> Result<Vec<Utf8PathBuf>> {
    let search_dirs = get_firmware_search_dirs();
    let mut descriptors = Vec::new();

    let root = Dir::open_ambient_dir("/", cap_std_ext::cap_std::ambient_authority())?;

    for dir_path in search_dirs {
        // Strip leading slash to make path relative to root Dir
        let relative_path = dir_path
            .as_str()
            .strip_prefix('/')
            .unwrap_or(dir_path.as_str());
        // Use open_dir_optional to handle non-existent directories gracefully
        let Some(dir) = root.open_dir_optional(relative_path)? else {
            continue;
        };

        // Use walk with sort_by_file_name to get entries in lexical order per QEMU firmware interop spec
        let config = WalkConfiguration::default().sort_by_file_name();
        dir.walk::<_, color_eyre::eyre::Error>(&config, |component| {
            // Only process regular files with .json extension at the top level
            if component.file_type.is_file() {
                if let Some(ext) = component.path.extension() {
                    if ext == "json" {
                        let full_path = dir_path.join(
                            Utf8Path::from_path(component.path)
                                .ok_or_else(|| eyre!("Non-UTF8 path: {:?}", component.path))?,
                        );
                        descriptors.push(full_path);
                    }
                }
            }
            Ok(ControlFlow::Continue(()))
        })?;
    }

    Ok(descriptors)
}

/// Load and parse a firmware descriptor JSON file
pub(crate) fn load_firmware_descriptor(path: &Utf8Path) -> Result<FirmwareDescriptor> {
    let content = fs::read_to_string(path)
        .map_err(|e| eyre!("Failed to read firmware descriptor {}: {}", path, e))?;

    serde_json::from_str(&content)
        .map_err(|e| eyre!("Failed to parse firmware descriptor {}: {}", path, e))
}

/// Get the current architecture in QEMU format
pub(crate) fn get_qemu_architecture() -> &'static str {
    match std::env::consts::ARCH {
        "powerpc64" => "ppc64",
        "powerpc64le" => "ppc64le",
        arch => arch,
    }
}

/// Find firmware using QEMU firmware interop JSON descriptors
///
/// This follows the same approach as systemd-vmspawn:
/// - Searches in $XDG_CONFIG_HOME/qemu/firmware, /etc/qemu/firmware, /usr/share/qemu/firmware
/// - Filters by architecture and secure boot support
/// - Skips firmware with enrolled keys (known to cause issues)
fn find_firmware_from_descriptors(require_secure_boot: bool) -> Result<FirmwareInfo> {
    let descriptors = list_firmware_descriptors()?;
    let arch = get_qemu_architecture();

    for descriptor_path in descriptors {
        let descriptor = load_firmware_descriptor(&descriptor_path)?;

        // Skip firmware with enrolled keys (known to cause issues)
        if descriptor.has_enrolled_keys() {
            tracing::debug!(
                "Skipping {}, firmware has enrolled keys which has been known to cause issues",
                descriptor_path
            );
            continue;
        }

        // Check architecture support
        if !descriptor.supports_architecture(arch) {
            tracing::debug!(
                "Skipping {}, firmware doesn't support architecture {}",
                descriptor_path,
                arch
            );
            continue;
        }

        // Check secure boot requirement
        if require_secure_boot && !descriptor.supports_secure_boot() {
            tracing::debug!(
                "Skipping {}, firmware doesn't support secure boot",
                descriptor_path
            );
            continue;
        }

        // Skip memory-mapped firmware (we need separate code and vars files)
        if descriptor.mapping.device == FirmwareMapping::DEVICE_TYPE_MEMORY {
            tracing::debug!(
                "Skipping {}, memory-mapped firmware not supported",
                descriptor_path
            );
            continue;
        }

        // Extract code and vars paths and formats from flash device firmware
        let firmware_info = match (
            &descriptor.mapping.executable,
            &descriptor.mapping.nvram_template,
        ) {
            (Some(executable), Some(nvram_template)) => FirmwareInfo {
                code_path: Utf8PathBuf::from(&executable.filename),
                code_format: executable.format.clone(),
                vars_path: Utf8PathBuf::from(&nvram_template.filename),
                vars_format: nvram_template.format.clone(),
            },
            _ => {
                tracing::debug!(
                    "Skipping {}, missing executable or nvram-template fields",
                    descriptor_path
                );
                continue;
            }
        };

        tracing::debug!("Selected firmware definition {}", descriptor_path);
        return Ok(firmware_info);
    }

    Err(eyre!(
        "No suitable firmware descriptor found for architecture {} with secure_boot={}",
        arch,
        require_secure_boot
    ))
}

/// Find the system OVMF_VARS.fd file using QEMU firmware interop JSON descriptors
pub(crate) fn find_ovmf_vars() -> Result<Utf8PathBuf> {
    let firmware_info = find_firmware_from_descriptors(false)?;

    if !firmware_info.vars_path.exists() {
        return Err(eyre!(
            "Firmware descriptor returned non-existent path: {}. Please verify your QEMU firmware installation.",
            firmware_info.vars_path
        ));
    }

    tracing::debug!(
        "Found OVMF_VARS via firmware descriptor: {}",
        firmware_info.vars_path
    );
    Ok(firmware_info.vars_path)
}

/// Find secure boot firmware using QEMU firmware interop JSON descriptors
///
/// Returns full firmware info including paths and formats for both CODE and VARS
pub fn find_secure_boot_firmware() -> Result<FirmwareInfo> {
    let firmware_info = find_firmware_from_descriptors(true)?;

    if !firmware_info.code_path.exists() {
        return Err(eyre!(
            "Firmware descriptor returned non-existent path: {}. Please verify your QEMU firmware installation.",
            firmware_info.code_path
        ));
    }

    tracing::debug!(
        "Found secure boot firmware: code={} ({}), vars={} ({})",
        firmware_info.code_path,
        firmware_info.code_format,
        firmware_info.vars_path,
        firmware_info.vars_format
    );
    Ok(firmware_info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // Note: These tests use direct command execution

    #[test]
    fn test_owned_vars_template() {
        let domain = |nvram: &str| {
            crate::xml_utils::parse_xml_dom(&format!(
                "<domain><name>vm1</name><os>{nvram}</os></domain>"
            ))
            .unwrap()
        };
        let cases = [
            (
                r#"<nvram template="/pool/vm1_OVMF_VARS.fd" format="raw"/>"#,
                Some("/pool/vm1_OVMF_VARS.fd"),
            ),
            // Another domain's template
            (r#"<nvram template="/pool/vm2_OVMF_VARS.fd"/>"#, None),
            // The system template is never ours to delete
            (
                r#"<nvram template="/usr/share/edk2/ovmf/OVMF_VARS.secboot.fd"/>"#,
                None,
            ),
            (
                r#"<nvram>/var/lib/libvirt/qemu/nvram/vm1_VARS.fd</nvram>"#,
                None,
            ),
            ("", None),
        ];
        for (nvram, expected) in cases {
            assert_eq!(
                owned_vars_template(&domain(nvram), "vm1").as_deref(),
                expected.map(Utf8Path::new),
                "{nvram}"
            );
        }
    }

    #[test]
    fn test_load_missing_directory() {
        let temp_dir = TempDir::new().unwrap();
        let key_dir = Utf8PathBuf::try_from(temp_dir.path().join("nonexistent")).unwrap();

        let result = SecureBootKeys::load(&key_dir);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_load_incomplete_keys() {
        let temp_dir = TempDir::new().unwrap();
        let key_dir = Utf8PathBuf::try_from(temp_dir.path().join("keys")).unwrap();
        fs::create_dir_all(&key_dir).unwrap();

        // Create GUID but no certificates
        fs::write(key_dir.join("GUID.txt"), "test-guid").unwrap();

        let result = SecureBootKeys::load(&key_dir);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("PK.crt not found"));
    }
}
