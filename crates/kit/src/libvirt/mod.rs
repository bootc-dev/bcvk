//! libvirt integration for bcvk
//!
//! This module provides a comprehensive libvirt integration with subcommands for:
//! - `run`: Run a bootable container as a persistent VM
//! - `list`: List bootc domains with metadata
//! - `upload`: Upload bootc disk images to libvirt with metadata annotations
//! - `list-volumes`: List available bootc volumes with metadata

use clap::Subcommand;

/// Output format options for libvirt commands
#[derive(Debug, Clone, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum OutputFormat {
    Table,
    Json,
    Yaml,
    Xml,
}

/// Default memory allocation for libvirt VMs
pub const LIBVIRT_DEFAULT_MEMORY: &str = "4G";

/// Default disk size for libvirt base disks
pub const LIBVIRT_DEFAULT_DISK_SIZE: &str = "20G";

pub mod base_disks;
pub mod base_disks_cli;
pub mod domain;
pub mod inspect;
pub mod list;
pub mod list_volumes;
pub mod print_firmware;
pub mod rm;
pub mod rm_all;
pub mod run;
pub mod run_anaconda;
pub mod secureboot;
pub mod ssh;
pub mod start;
pub mod status;
pub mod stop;
pub mod upload;

/// Global options for libvirt operations
#[derive(Debug, Clone, Default)]
pub struct LibvirtOptions {
    /// Hypervisor connection URI (e.g., qemu:///system, qemu+ssh://host/system)
    pub connect: Option<String>,
}

impl LibvirtOptions {
    /// Create a virsh Command with the appropriate connection URI
    pub fn virsh_command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new("virsh");
        if let Some(ref uri) = self.connect {
            cmd.arg("-c").arg(uri);
        }
        cmd
    }
}

/// Convert a unit string to bytes multiplier
/// Handles libvirt-style units distinguishing between decimal (KB, MB, GB - powers of 1000)
/// and binary (KiB, MiB, GiB - powers of 1024) units per libvirt specification
pub(crate) fn unit_to_bytes(unit: &str) -> Option<u128> {
    match unit {
        // Binary prefixes (powers of 1024)
        "B" | "bytes" => Some(1),
        "k" | "K" | "KiB" => Some(1024),
        "M" | "MiB" => Some(1024u128.pow(2)),
        "G" | "GiB" => Some(1024u128.pow(3)),
        "T" | "TiB" => Some(1024u128.pow(4)),

        // Decimal prefixes (powers of 1000)
        "KB" => Some(1_000),
        "MB" => Some(1_000u128.pow(2)),
        "GB" => Some(1_000u128.pow(3)),
        "TB" => Some(1_000u128.pow(4)),

        _ => None,
    }
}

/// Convert memory value with unit to megabytes (MiB)
/// Handles libvirt-style units distinguishing between decimal (KB, MB, GB - powers of 1000)
/// and binary (KiB, MiB, GiB - powers of 1024) units per libvirt specification
/// Returns None if the unit is unknown or if the result overflows u32
pub(crate) fn convert_memory_to_mb(value: u32, unit: &str) -> Option<u32> {
    let value_u128 = value as u128;
    let mib_u128 = 1024 * 1024;

    // Convert to bytes first, then to MiB
    let bytes = value_u128 * unit_to_bytes(unit)?;
    let mb = bytes / mib_u128;

    u32::try_from(mb).ok()
}

/// Convert memory value with unit to megabytes (MiB), returning u64
/// Handles libvirt-style units distinguishing between decimal (KB, MB, GB - powers of 1000)
/// and binary (KiB, MiB, GiB - powers of 1024) units per libvirt specification
/// Returns None if the unit is unknown or if the result overflows u64
#[allow(dead_code)]
pub(crate) fn convert_to_mb(value: u64, unit: &str) -> Option<u64> {
    let value_u128 = value as u128;
    let mib_u128 = 1024 * 1024;

    // Convert to bytes first, then to MiB
    let bytes = value_u128 * unit_to_bytes(unit)?;
    let mb = bytes / mib_u128;

    u64::try_from(mb).ok()
}

/// Parse memory value from a libvirt XML node with unit attribute
/// Returns the value in megabytes (MiB)
pub(crate) fn parse_memory_mb(node: &crate::xml_utils::XmlNode) -> Option<u32> {
    let value = node.text_content().parse::<u32>().ok()?;
    // Convert to MB based on unit attribute (default is KiB per libvirt spec)
    let unit = node
        .attributes
        .get("unit")
        .map(|s| s.as_str())
        .unwrap_or("KiB");
    convert_memory_to_mb(value, unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_memory_to_mb() {
        // Test binary units (powers of 1024)
        assert_eq!(convert_memory_to_mb(4194304, "KiB"), Some(4096));
        assert_eq!(convert_memory_to_mb(2097152, "KiB"), Some(2048));
        assert_eq!(convert_memory_to_mb(2048, "MiB"), Some(2048));
        assert_eq!(convert_memory_to_mb(4096, "MiB"), Some(4096));
        assert_eq!(convert_memory_to_mb(4, "GiB"), Some(4096));
        assert_eq!(convert_memory_to_mb(2, "GiB"), Some(2048));

        // Test short forms (binary)
        assert_eq!(convert_memory_to_mb(4, "G"), Some(4096));
        assert_eq!(convert_memory_to_mb(2048, "M"), Some(2048));
        assert_eq!(convert_memory_to_mb(2097152, "K"), Some(2048));

        // Test decimal units (powers of 1000)
        assert_eq!(convert_memory_to_mb(1048576, "KB"), Some(1000));
        assert_eq!(convert_memory_to_mb(1024, "MB"), Some(976));
        assert_eq!(convert_memory_to_mb(4, "GB"), Some(3814));

        // Test unknown unit returns None
        assert_eq!(convert_memory_to_mb(4194304, "unknown"), None);
    }

    #[test]
    fn test_parse_memory_mb() {
        use crate::xml_utils::parse_xml_dom;

        // Test KiB (default unit)
        let xml = r#"<memory>4194304</memory>"#;
        let dom = parse_xml_dom(xml).unwrap();
        assert_eq!(parse_memory_mb(&dom), Some(4096));

        // Test MiB
        let xml = r#"<memory unit='MiB'>2048</memory>"#;
        let dom = parse_xml_dom(xml).unwrap();
        assert_eq!(parse_memory_mb(&dom), Some(2048));

        // Test GiB
        let xml = r#"<memory unit='GiB'>4</memory>"#;
        let dom = parse_xml_dom(xml).unwrap();
        assert_eq!(parse_memory_mb(&dom), Some(4096));

        // Test KB (decimal unit: 1000-based)
        let xml = r#"<memory unit='KB'>1048576</memory>"#;
        let dom = parse_xml_dom(xml).unwrap();
        assert_eq!(parse_memory_mb(&dom), Some(1000));
    }
}

/// libvirt subcommands for managing bootc disk images and domains
#[derive(Debug, Subcommand)]
pub enum LibvirtSubcommands {
    /// Run a bootable container as a persistent VM
    Run(run::LibvirtRunOpts),

    /// Run a bootable container as a persistent VM, installed via anaconda
    #[clap(name = "run-anaconda")]
    RunAnaconda(run_anaconda::LibvirtRunAnacondaOpts),

    /// SSH to libvirt domain with embedded SSH key
    Ssh(ssh::LibvirtSshOpts),

    /// List bootc domains with metadata
    List(list::LibvirtListOpts),

    /// List available bootc volumes with metadata
    #[clap(name = "list-volumes")]
    ListVolumes(list_volumes::LibvirtListVolumesOpts),

    /// Stop a running libvirt domain
    Stop(stop::LibvirtStopOpts),

    /// Start a stopped libvirt domain
    Start(start::LibvirtStartOpts),

    /// Remove a libvirt domain and its resources
    #[clap(name = "rm")]
    Remove(rm::LibvirtRmOpts),

    /// Remove multiple libvirt domains and their resources
    #[clap(name = "rm-all")]
    RemoveAll(rm_all::LibvirtRmAllOpts),

    /// Show detailed information about a libvirt domain
    Inspect(inspect::LibvirtInspectOpts),

    /// Show libvirt environment status and capabilities
    Status(status::LibvirtStatusOpts),

    /// Upload bootc disk images to libvirt with metadata annotations
    Upload(upload::LibvirtUploadOpts),

    /// Manage base disk images used for VM cloning
    #[clap(name = "base-disks")]
    BaseDisks(base_disks_cli::LibvirtBaseDisksOpts),

    /// Print detected firmware paths and configuration
    #[clap(name = "print-firmware", hide = true)]
    PrintFirmware(print_firmware::LibvirtPrintFirmwareOpts),
}
