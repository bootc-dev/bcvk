//! Secure Boot key management for libvirt/QEMU virtualization
//!
//! This module provides utilities for loading existing UEFI Secure Boot
//! keys (PK, KEK, db) and customizing OVMF firmware variables for VMs.

use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::{eyre::eyre, Result};

/// Secure Boot key configuration
#[derive(Debug, Clone)]
pub struct SecureBootConfig {
    /// Directory containing the secure boot keys
    pub key_dir: Utf8PathBuf,
    /// Path to custom OVMF_VARS template with enrolled keys
    pub vars_template: Utf8PathBuf,
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
        return Err(eyre!(
            "virt-fw-vars not found. Install it with: dnf install -y python3-virt-firmware"
        ));
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

/// Load and setup secure boot configuration from existing keys
pub fn setup_secure_boot(key_dir: &Utf8Path) -> Result<SecureBootConfig> {
    tracing::info!("Loading secure boot keys from {}", key_dir);
    let keys = SecureBootKeys::load(key_dir)?;

    // Path for the customized OVMF_VARS template
    let vars_template = key_dir.join("OVMF_VARS_CUSTOM.fd");

    // Find the system OVMF_VARS.fd
    let ovmf_vars = find_ovmf_vars()?;

    // Check if custom vars template already exists
    let mut test_template = std::process::Command::new("test");
    test_template.args(["-f", vars_template.as_str()]);

    if !test_template.status()?.success() {
        tracing::info!("Creating custom OVMF_VARS template with enrolled keys");
        customize_ovmf_vars(&keys, &ovmf_vars, &vars_template)?;
    }

    Ok(SecureBootConfig {
        key_dir: key_dir.to_owned(),
        vars_template,
        guid: keys.guid,
    })
}

/// Find the system OVMF_VARS.fd file
fn find_ovmf_vars() -> Result<Utf8PathBuf> {
    // Common locations for OVMF_VARS.fd
    let locations = [
        "/usr/share/edk2/ovmf/OVMF_VARS.fd",
        "/usr/share/OVMF/OVMF_VARS.fd",
        "/usr/share/qemu/OVMF_VARS.fd",
        "/usr/share/edk2-ovmf/OVMF_VARS.fd",
    ];

    for path in &locations {
        let mut test_file = std::process::Command::new("test");
        test_file.args(["-f", path]);

        if test_file.status()?.success() {
            return Ok(Utf8PathBuf::from(path));
        }
    }

    Err(eyre!(
        "Could not find OVMF_VARS.fd. Please install edk2-ovmf package."
    ))
}

/// Find the secure boot OVMF_CODE file
pub fn find_ovmf_code_secboot() -> Result<Utf8PathBuf> {
    // Common locations for OVMF_CODE.secboot.fd
    let locations = [
        "/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd",
        "/usr/share/OVMF/OVMF_CODE.secboot.fd",
        "/usr/share/qemu/OVMF_CODE.secboot.fd",
        "/usr/share/edk2-ovmf/OVMF_CODE.secboot.fd",
    ];

    for path in &locations {
        let mut test_file = std::process::Command::new("test");
        test_file.args(["-f", path]);

        if test_file.status()?.success() {
            return Ok(Utf8PathBuf::from(path));
        }
    }

    Err(eyre!(
        "Could not find OVMF_CODE.secboot.fd. Please install edk2-ovmf package."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // Note: These tests use direct command execution

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
