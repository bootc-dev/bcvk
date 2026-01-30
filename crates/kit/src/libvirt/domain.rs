//! Domain XML generation and management utilities
//!
//! This module provides utilities for generating libvirt domain XML configurations
//! for bootc containers, inspired by the podman-bootc domain builder pattern.

use crate::arch::ArchConfig;
use crate::common_opts::DEFAULT_MEMORY_USER_STR;
use crate::libvirt::run::FirmwareType;
use crate::run_ephemeral::default_vcpus;
use crate::xml_utils::XmlWriter;
use color_eyre::{eyre::eyre, Result};
use std::collections::HashMap;
use uuid::Uuid;

/// Configuration for a virtiofs filesystem mount
#[derive(Debug, Clone)]
pub struct VirtiofsFilesystem {
    /// Host directory to share
    pub source_dir: String,
    /// Unique tag identifier for the filesystem
    pub tag: String,
    /// Whether the filesystem is read-only
    pub readonly: bool,
}

/// Configuration for firmware debug log output
#[derive(Debug, Clone)]
pub enum FirmwareLogOutput {
    /// Write firmware log to a file on the host
    #[allow(dead_code)]
    File(String),
    /// Make firmware log available via virsh console (pty)
    Console,
}

/// Builder for creating libvirt domain XML configurations
#[derive(Debug)]
pub struct DomainBuilder {
    name: Option<String>,
    uuid: Option<String>,
    memory: Option<u64>, // in MB
    vcpus: Option<u32>,
    disk_path: Option<String>,
    transient_disk: bool, // Use transient disk with temporary overlay
    network: Option<String>,
    vnc_port: Option<u16>,
    kernel_args: Option<String>,
    metadata: HashMap<String, String>,
    qemu_args: Vec<String>,
    virtiofs_filesystems: Vec<VirtiofsFilesystem>,
    firmware: Option<FirmwareType>,
    tpm: bool,
    ovmf_code_path: Option<String>, // Custom OVMF_CODE path for secure boot
    ovmf_code_format: Option<String>, // Format of OVMF_CODE (raw, qcow2)
    nvram_template: Option<String>, // Custom NVRAM template with enrolled keys
    nvram_format: Option<String>,   // Format of NVRAM template (raw, qcow2)
    firmware_log: Option<FirmwareLogOutput>, // OVMF debug log output via isa-debugcon
}

impl Default for DomainBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl DomainBuilder {
    /// Create a new domain builder
    pub fn new() -> Self {
        Self {
            name: None,
            uuid: None,
            memory: None,
            vcpus: None,
            disk_path: None,
            transient_disk: false,
            network: None,
            vnc_port: None,
            kernel_args: None,
            metadata: HashMap::new(),
            qemu_args: Vec::new(),
            virtiofs_filesystems: Vec::new(),
            firmware: None, // Defaults to UEFI
            tpm: true,      // Default to enabled
            ovmf_code_path: None,
            ovmf_code_format: None,
            nvram_template: None,
            nvram_format: None,
            firmware_log: Some(FirmwareLogOutput::Console), // Default to pty for virsh console access
        }
    }

    /// Set domain name
    pub fn with_name(mut self, name: &str) -> Self {
        self.name = Some(name.to_string());
        self
    }

    /// Set memory in MB
    pub fn with_memory(mut self, memory_mb: u64) -> Self {
        self.memory = Some(memory_mb);
        self
    }

    /// Set number of vCPUs
    pub fn with_vcpus(mut self, vcpus: u32) -> Self {
        self.vcpus = Some(vcpus);
        self
    }

    /// Set disk path
    pub fn with_disk(mut self, disk_path: &str) -> Self {
        self.disk_path = Some(disk_path.to_string());
        self
    }

    /// Enable transient disk (creates temporary overlay, base disk opened read-only)
    pub fn with_transient_disk(mut self, transient: bool) -> Self {
        self.transient_disk = transient;
        self
    }

    /// Set network configuration
    pub fn with_network(mut self, network: &str) -> Self {
        self.network = Some(network.to_string());
        self
    }

    /// Enable VNC on specified port
    #[allow(dead_code)]
    pub fn with_vnc(mut self, port: u16) -> Self {
        self.vnc_port = Some(port);
        self
    }

    /// Set kernel arguments for direct boot
    #[allow(dead_code)]
    pub fn with_kernel_args(mut self, kernel_args: &str) -> Self {
        self.kernel_args = Some(kernel_args.to_string());
        self
    }

    /// Add metadata key-value pair
    pub fn with_metadata(mut self, key: &str, value: &str) -> Self {
        self.metadata.insert(key.to_string(), value.to_string());
        self
    }

    /// Add QEMU command line arguments
    pub fn with_qemu_args(mut self, args: Vec<String>) -> Self {
        self.qemu_args = args;
        self
    }

    /// Add a virtiofs filesystem mount
    pub fn with_virtiofs_filesystem(mut self, filesystem: VirtiofsFilesystem) -> Self {
        self.virtiofs_filesystems.push(filesystem);
        self
    }

    /// Set firmware type (defaults to uefi-secure)
    pub fn with_firmware(mut self, firmware: FirmwareType) -> Self {
        self.firmware = Some(firmware);
        self
    }

    /// Enable TPM 2.0 support using swtpm
    pub fn with_tpm(mut self, tpm: bool) -> Self {
        self.tpm = tpm;
        self
    }

    /// Set custom OVMF_CODE path and format for secure boot
    ///
    /// Format must be specified (either "raw" or "qcow2") and should come from
    /// the QEMU firmware interop JSON descriptors.
    pub fn with_ovmf_code_path(mut self, path: &str, format: &str) -> Self {
        self.ovmf_code_path = Some(path.to_string());
        self.ovmf_code_format = Some(format.to_string());
        self
    }

    /// Set custom NVRAM template path and format with enrolled secure boot keys
    ///
    /// Format must be specified (either "raw" or "qcow2") and should come from
    /// the QEMU firmware interop JSON descriptors.
    pub fn with_nvram_template(mut self, path: &str, format: &str) -> Self {
        self.nvram_template = Some(path.to_string());
        self.nvram_format = Some(format.to_string());
        self
    }

    /// Enable firmware debug log output via isa-debugcon (x86_64 only)
    ///
    /// This captures OVMF/EDK2 DEBUG() output which is useful for debugging
    /// Secure Boot failures and other firmware issues. The log is available
    /// on IO port 0x402.
    ///
    /// Options:
    /// - `FirmwareLogOutput::File(path)` - Write to a file on the host
    /// - `FirmwareLogOutput::Console` - Access via `virsh console <domain> serial1`
    #[allow(dead_code)]
    pub fn with_firmware_log(mut self, output: FirmwareLogOutput) -> Self {
        self.firmware_log = Some(output);
        self
    }

    /// Build the domain XML
    pub fn build_xml(self) -> Result<String> {
        let name = self.name.ok_or_else(|| eyre!("Domain name is required"))?;
        let memory = self.memory.unwrap_or_else(|| {
            crate::utils::parse_memory_to_mb(DEFAULT_MEMORY_USER_STR)
                .unwrap()
                .into()
        });
        let vcpus = self.vcpus.unwrap_or_else(default_vcpus);
        let uuid = self.uuid.unwrap_or_else(|| Uuid::new_v4().to_string());

        // Detect architecture configuration
        let arch_config = ArchConfig::detect()?;

        let mut writer = XmlWriter::new();

        // Root domain element
        let domain_attrs = if self.qemu_args.is_empty() {
            vec![("type", "kvm")]
        } else {
            vec![
                ("type", "kvm"),
                ("xmlns:qemu", "http://libvirt.org/schemas/domain/qemu/1.0"),
            ]
        };
        writer.start_element("domain", &domain_attrs)?;

        // Basic domain information
        writer.write_text_element("name", &name)?;
        writer.write_text_element("uuid", &uuid)?;
        writer.write_text_element_with_attrs("memory", &memory.to_string(), &[("unit", "MiB")])?;
        writer.write_text_element_with_attrs(
            "currentMemory",
            &memory.to_string(),
            &[("unit", "MiB")],
        )?;
        writer.write_text_element("vcpu", &vcpus.to_string())?;

        // OS section with firmware configuration
        let use_uefi = self.firmware != Some(FirmwareType::Bios);
        let secure_boot = use_uefi
            && (self.firmware == Some(FirmwareType::UefiSecure) || self.ovmf_code_path.is_some());
        let insecure_boot = self.firmware == Some(FirmwareType::UefiInsecure);

        // Don't use firmware="efi" when we have custom OVMF paths (secure boot with custom keys)
        // because firmware="efi" and explicit <loader> paths are mutually exclusive
        let os_attributes = (use_uefi && self.ovmf_code_path.is_none())
            .then_some([("firmware", "efi")].as_slice())
            .unwrap_or_default();
        writer.start_element("os", os_attributes)?;

        // For secure boot on x86_64, we may need a specific machine type with SMM
        let machine_type = if secure_boot && arch_config.arch == "x86_64" {
            "q35" // Modern libvirt will handle SMM automatically with q35
        } else {
            arch_config.machine
        };

        writer.write_text_element_with_attrs(
            "type",
            &arch_config.os_type,
            &[("arch", &arch_config.arch), ("machine", machine_type)],
        )?;

        if use_uefi {
            if let Some(ref ovmf_code) = self.ovmf_code_path {
                // Use custom OVMF_CODE path for secure boot
                // Format is required and comes from QEMU firmware interop JSON descriptors
                let code_format = self
                    .ovmf_code_format
                    .as_deref()
                    .expect("ovmf_code_format must be set when ovmf_code_path is set");
                let mut loader_attrs = vec![
                    ("readonly", "yes"),
                    ("type", "pflash"),
                    ("format", code_format),
                ];
                if secure_boot {
                    loader_attrs.push(("secure", "yes"));
                }
                writer.write_text_element_with_attrs("loader", ovmf_code, &loader_attrs)?;

                // Add NVRAM element if template is specified
                if let Some(ref nvram_template) = self.nvram_template {
                    // Format is required and comes from QEMU firmware interop JSON descriptors
                    let nvram_fmt = self
                        .nvram_format
                        .as_deref()
                        .expect("nvram_format must be set when nvram_template is set");
                    writer.write_text_element_with_attrs(
                        "nvram",
                        "", // Empty content, template attr provides the source
                        &[
                            ("template", nvram_template),
                            ("templateFormat", nvram_fmt),
                            ("format", nvram_fmt),
                        ],
                    )?;
                }
            } else if secure_boot {
                // Let libvirt auto-select firmware for secure boot
                writer.write_empty_element("loader", &[("secure", "yes")])?;
            } else if insecure_boot {
                // Explicitly disable secure boot for uefi-insecure
                writer.write_empty_element("loader", &[("secure", "no")])?;
            }
        }

        writer.write_empty_element("boot", &[("dev", "hd")])?;

        // Add kernel arguments if specified (for direct boot)
        if let Some(ref kargs) = self.kernel_args {
            writer.write_text_element("cmdline", kargs)?;
        }

        writer.end_element("os")?;

        // Add memory backing for shared memory support (required for virtiofs)
        writer.start_element("memoryBacking", &[])?;
        writer.write_empty_element("source", &[("type", "memfd")])?;
        writer.write_empty_element("access", &[("mode", "shared")])?;
        writer.end_element("memoryBacking")?;

        // Write features including SMM for secure boot
        writer.start_element("features", &[])?;
        writer.write_empty_element("acpi", &[])?;
        writer.write_empty_element("apic", &[])?;

        // Add x86_64-specific features
        if arch_config.arch == "x86_64" {
            writer.write_empty_element("vmport", &[("state", "off")])?;
            // Add SMM support for secure boot on x86_64
            if secure_boot {
                writer.write_empty_element("smm", &[("state", "on")])?;
            }
        }

        writer.end_element("features")?;

        // Architecture-specific CPU configuration
        writer.write_empty_element("cpu", &[("mode", arch_config.cpu_mode())])?;

        // Clock and lifecycle configuration
        writer.start_element("clock", &[("offset", "utc")])?;
        arch_config.write_timers(&mut writer)?;
        writer.end_element("clock")?;

        writer.write_text_element("on_poweroff", "destroy")?;
        writer.write_text_element("on_reboot", "restart")?;
        writer.write_text_element("on_crash", "destroy")?;

        // Devices section
        writer.start_element("devices", &[])?;

        // Disk
        if let Some(ref disk_path) = self.disk_path {
            // Auto-detect disk format from file extension
            let disk_type = if disk_path.ends_with(".qcow2") {
                "qcow2"
            } else {
                "raw"
            };

            writer.start_element("disk", &[("type", "file"), ("device", "disk")])?;
            writer.write_empty_element("driver", &[("name", "qemu"), ("type", disk_type)])?;
            writer.write_empty_element("source", &[("file", disk_path)])?;
            writer.write_empty_element("target", &[("dev", "vda"), ("bus", "virtio")])?;
            if self.transient_disk {
                // shareBacking='yes' allows multiple VMs to share the backing image
                // Libvirt creates a temporary QCOW2 overlay for writes
                writer.write_empty_element("transient", &[("shareBacking", "yes")])?;
            }
            writer.end_element("disk")?;
        }

        // Network
        let network_config = self.network.as_deref().unwrap_or("default");
        match network_config {
            "none" => {
                // No network interface
            }
            "default" => {
                // Skip explicit network interface - let libvirt use its default behavior
                // This avoids issues when the "default" network doesn't exist
            }
            "user" => {
                // User-mode networking (NAT) - no network name required
                writer.start_element("interface", &[("type", "user")])?;
                writer.write_empty_element("model", &[("type", "virtio")])?;
                writer.end_element("interface")?;
            }
            network if network.starts_with("bridge=") => {
                let bridge_name = network.strip_prefix("bridge=").unwrap();
                writer.start_element("interface", &[("type", "bridge")])?;
                writer.write_empty_element("source", &[("bridge", bridge_name)])?;
                writer.write_empty_element("model", &[("type", "virtio")])?;
                writer.end_element("interface")?;
            }
            _ => {
                // Assume it's a network name
                writer.start_element("interface", &[("type", "network")])?;
                writer.write_empty_element("source", &[("network", network_config)])?;
                writer.write_empty_element("model", &[("type", "virtio")])?;
                writer.end_element("interface")?;
            }
        }

        // Serial console, see https://libvirt.org/formatdomain.html#relationship-between-serial-ports-and-consoles
        // We allocate a platform-specific default for early console stuff like bootloaders,
        // and a platform-independent `hvc0` that can be referenced independently.
        writer.start_element("console", &[("type", "pty")])?;
        writer.write_empty_element("target", &[("type", "serial")])?;
        writer.end_element("console")?;
        writer.start_element("console", &[("type", "pty")])?;
        writer.write_empty_element("target", &[("type", "virtio")])?;
        writer.end_element("console")?;

        // Firmware debug log via isa-debugcon (x86_64 only)
        // This captures OVMF/EDK2 DEBUG() output on IO port 0x402, useful for
        // debugging Secure Boot failures. Access via: virsh console <domain> serial0
        // See: https://libvirt.org/formatdomain.html#serial-port (isa-debug target type)
        if arch_config.arch == "x86_64" {
            if let Some(ref firmware_log) = self.firmware_log {
                let (serial_type, source_path) = match firmware_log {
                    FirmwareLogOutput::Console => ("pty", None),
                    FirmwareLogOutput::File(path) => ("file", Some(path.as_str())),
                };

                writer.start_element("serial", &[("type", serial_type)])?;
                if let Some(path) = source_path {
                    writer.write_empty_element("source", &[("path", path)])?;
                }
                writer.start_element("target", &[("type", "isa-debug"), ("port", "0")])?;
                writer.write_empty_element("model", &[("name", "isa-debugcon")])?;
                writer.end_element("target")?;
                writer.write_empty_element("address", &[("type", "isa"), ("iobase", "0x402")])?;
                writer.end_element("serial")?;
            }
        }

        // VNC graphics if enabled
        if let Some(vnc_port) = self.vnc_port {
            writer.write_empty_element(
                "graphics",
                &[
                    ("type", "vnc"),
                    ("port", &vnc_port.to_string()),
                    ("listen", "127.0.0.1"),
                ],
            )?;
            writer.start_element("video", &[])?;
            writer.write_empty_element("model", &[("type", "vga")])?;
            writer.end_element("video")?;
        }

        // SPICE graphics for virt-manager access
        // Always enable SPICE with auto-port allocation for easy debugging via virt-manager
        writer.start_element("graphics", &[("type", "spice"), ("autoport", "yes")])?;
        writer.write_empty_element("listen", &[("type", "address"), ("address", "127.0.0.1")])?;
        writer.end_element("graphics")?;
        writer.start_element("video", &[])?;
        writer.write_empty_element("model", &[("type", "virtio")])?;
        writer.end_element("video")?;

        // Virtiofs filesystems
        for filesystem in &self.virtiofs_filesystems {
            writer.start_element(
                "filesystem",
                &[("type", "mount"), ("accessmode", "passthrough")],
            )?;
            writer.write_empty_element("driver", &[("type", "virtiofs"), ("queue", "1024")])?;
            if filesystem.readonly {
                writer.write_empty_element("readonly", &[])?;
            }
            writer.write_empty_element("source", &[("dir", &filesystem.source_dir)])?;
            writer.write_empty_element("target", &[("dir", &filesystem.tag)])?;
            writer.end_element("filesystem")?;
        }

        // TPM device
        if self.tpm {
            writer.start_element("tpm", &[("model", "tpm-tis")])?;
            writer.write_empty_element("backend", &[("type", "emulator"), ("version", "2.0")])?;
            writer.end_element("tpm")?;
        }

        writer.end_element("devices")?;

        // QEMU commandline section (if we have QEMU args)
        if !self.qemu_args.is_empty() {
            writer.start_element("qemu:commandline", &[])?;
            for arg in &self.qemu_args {
                writer.write_empty_element("qemu:arg", &[("value", arg)])?;
            }
            writer.end_element("qemu:commandline")?;
        }

        // Metadata section
        if !self.metadata.is_empty() {
            writer.start_element("metadata", &[])?;
            writer.start_element(
                "bootc:container",
                &[("xmlns:bootc", "https://github.com/containers/bootc")],
            )?;

            for (key, value) in &self.metadata {
                // Ensure the key has the bootc: prefix
                let element_name = if key.starts_with("bootc:") {
                    key.clone()
                } else {
                    format!("bootc:{}", key)
                };
                writer.write_text_element(&element_name, value)?;
            }

            writer.end_element("bootc:container")?;
            writer.end_element("metadata")?;
        }

        writer.end_element("domain")?;

        writer.into_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_domain_xml() {
        let xml = DomainBuilder::new()
            .with_name("test-domain")
            .with_memory(4096)
            .with_vcpus(4)
            .with_disk("/path/to/disk.raw")
            .build_xml()
            .unwrap();

        assert!(xml.contains("<name>test-domain</name>"));
        assert!(xml.contains("<memory unit=\"MiB\">4096</memory>"));
        assert!(xml.contains("<vcpu>4</vcpu>"));
        assert!(xml.contains("source file=\"/path/to/disk.raw\""));

        // Should contain current architecture (detected at runtime)
        let arch = std::env::consts::ARCH;
        assert!(xml.contains(&format!("arch=\"{}\"", arch)));

        // Libvirt will automatically detect the appropriate emulator
    }

    #[test]
    fn test_domain_with_metadata() {
        let xml = DomainBuilder::new()
            .with_name("test-domain")
            .with_metadata("bootc:source-image", "quay.io/fedora/fedora-bootc:42")
            .with_metadata("bootc:filesystem", "xfs")
            .build_xml()
            .unwrap();

        assert!(xml.contains("bootc:container"));
        assert!(
            xml.contains("<bootc:source-image>quay.io/fedora/fedora-bootc:42</bootc:source-image>")
        );
        assert!(xml.contains("<bootc:filesystem>xfs</bootc:filesystem>"));
    }

    #[test]
    fn test_network_configurations() {
        // Default network - should not add explicit interface
        let xml = DomainBuilder::new()
            .with_name("test")
            .with_network("default")
            .build_xml()
            .unwrap();
        assert!(!xml.contains("source network=\"default\""));

        // Bridge network
        let xml = DomainBuilder::new()
            .with_name("test")
            .with_network("bridge=virbr0")
            .build_xml()
            .unwrap();
        assert!(xml.contains("source bridge=\"virbr0\""));

        // No network
        let xml = DomainBuilder::new()
            .with_name("test")
            .with_network("none")
            .build_xml()
            .unwrap();
        assert!(!xml.contains("<interface"));
    }

    #[test]
    fn test_vnc_configuration() {
        let xml = DomainBuilder::new()
            .with_name("test")
            .with_vnc(5901)
            .build_xml()
            .unwrap();

        assert!(xml.contains("graphics type=\"vnc\" port=\"5901\""));
        assert!(xml.contains("model type=\"vga\""));
    }

    #[test]
    fn test_architecture_detection() {
        let xml = DomainBuilder::new()
            .with_name("test-arch")
            .build_xml()
            .unwrap();

        let host_arch = std::env::consts::ARCH;

        // Should contain the correct architecture
        assert!(xml.contains(&format!("arch=\"{}\"", host_arch)));

        // Should contain architecture-appropriate machine type
        match host_arch {
            "x86_64" => {
                assert!(xml.contains("machine=\"q35\""));
                assert!(xml.contains("vmport")); // x86_64-specific feature
                assert!(xml.contains("state=\"off\"")); // vmport should be disabled
            }
            "aarch64" => {
                assert!(xml.contains("machine=\"virt\""));
                assert!(!xml.contains("vmport")); // ARM64 doesn't have vmport
            }
            _ => {
                // Test passes for unsupported architectures (will use defaults)
            }
        }

        // Should contain architecture-specific features and timers
        assert!(xml.contains("<features>"));
        assert!(xml.contains("<acpi/>"));
        assert!(xml.contains("<timer name=\"rtc\""));
    }

    #[test]
    fn test_secure_boot_configuration() {
        let builder = DomainBuilder::new()
            .with_name("test-secure-boot")
            .with_firmware(FirmwareType::UefiSecure);

        let xml = builder.build_xml().unwrap();

        // Should include secure boot loader configuration
        assert!(xml.contains("loader"));
        assert!(xml.contains("secure=\"yes\""));

        // Should use firmware="efi" for UEFI
        assert!(xml.contains("firmware=\"efi\""));

        // Test UEFI insecure (secure boot explicitly disabled)
        let xml_insecure = DomainBuilder::new()
            .with_name("test-uefi-insecure")
            .with_firmware(FirmwareType::UefiInsecure)
            .build_xml()
            .unwrap();

        // Should use libvirt auto firmware selection with secure="no"
        assert!(xml_insecure.contains("firmware=\"efi\""));
        assert!(xml_insecure.contains("secure=\"no\""));
        assert!(!xml_insecure.contains("secure=\"yes\""));

        // Test BIOS firmware (no secure boot)
        let xml_bios = DomainBuilder::new()
            .with_name("test-bios")
            .with_firmware(FirmwareType::Bios)
            .build_xml()
            .unwrap();

        // Should not have firmware="efi" or secure boot settings
        assert!(!xml_bios.contains("firmware=\"efi\""));
        assert!(!xml_bios.contains("secure=\"yes\""));
    }

    #[test]
    fn test_tpm_configuration() {
        // Test TPM enabled (default)
        let xml = DomainBuilder::new()
            .with_name("test-tpm-enabled")
            .build_xml()
            .unwrap();

        // Should include TPM device by default
        assert!(xml.contains("<tpm model=\"tpm-tis\">"));
        assert!(xml.contains("<backend type=\"emulator\" version=\"2.0\"/>"));

        // Test TPM explicitly enabled
        let xml_enabled = DomainBuilder::new()
            .with_name("test-tpm-explicit")
            .with_tpm(true)
            .build_xml()
            .unwrap();

        assert!(xml_enabled.contains("<tpm model=\"tpm-tis\">"));
        assert!(xml_enabled.contains("backend type=\"emulator\""));

        // Test TPM disabled
        let xml_disabled = DomainBuilder::new()
            .with_name("test-tpm-disabled")
            .with_tpm(false)
            .build_xml()
            .unwrap();

        // Should not contain TPM configuration
        assert!(!xml_disabled.contains("<tpm"));
        assert!(!xml_disabled.contains("backend type=\"emulator\""));
    }

    #[test]
    fn test_secure_boot_with_custom_firmware() {
        let xml = DomainBuilder::new()
            .with_name("test-custom-secboot")
            .with_firmware(FirmwareType::UefiSecure)
            .with_ovmf_code_path("/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd", "raw")
            .with_nvram_template("/var/lib/libvirt/qemu/nvram/custom_VARS.fd", "raw")
            .build_xml()
            .unwrap();

        // Should have custom loader path
        assert!(xml.contains("/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd"));

        // Should have nvram template
        assert!(xml.contains("nvram"));
        assert!(xml.contains("template=\"/var/lib/libvirt/qemu/nvram/custom_VARS.fd\""));

        // Should have secure loader attributes
        assert!(xml.contains("readonly=\"yes\""));
        assert!(xml.contains("type=\"pflash\""));
        assert!(xml.contains("secure=\"yes\""));

        // Should have SMM enabled for x86_64
        if std::env::consts::ARCH == "x86_64" {
            assert!(xml.contains("<smm state=\"on\"/>"));
        }
    }

    #[test]
    fn test_virtiofs_filesystem_configuration() {
        // Test read-write virtiofs filesystem
        let filesystem_rw = VirtiofsFilesystem {
            source_dir: "/host/path".to_string(),
            tag: "testtag".to_string(),
            readonly: false,
        };

        let xml_rw = DomainBuilder::new()
            .with_name("test-virtiofs")
            .with_virtiofs_filesystem(filesystem_rw)
            .build_xml()
            .unwrap();

        assert!(xml_rw.contains("filesystem type=\"mount\" accessmode=\"passthrough\""));
        assert!(xml_rw.contains("driver type=\"virtiofs\" queue=\"1024\""));
        assert!(xml_rw.contains("source dir=\"/host/path\""));
        assert!(xml_rw.contains("target dir=\"testtag\""));
        assert!(!xml_rw.contains("<readonly/>"));

        // Test read-only virtiofs filesystem
        let filesystem_ro = VirtiofsFilesystem {
            source_dir: "/host/storage".to_string(),
            tag: "hoststorage".to_string(),
            readonly: true,
        };

        let xml_ro = DomainBuilder::new()
            .with_name("test-virtiofs-ro")
            .with_virtiofs_filesystem(filesystem_ro)
            .build_xml()
            .unwrap();

        assert!(xml_ro.contains("filesystem type=\"mount\" accessmode=\"passthrough\""));
        assert!(xml_ro.contains("driver type=\"virtiofs\" queue=\"1024\""));
        assert!(xml_ro.contains("<readonly/>"));
        assert!(xml_ro.contains("source dir=\"/host/storage\""));
        assert!(xml_ro.contains("target dir=\"hoststorage\""));
    }

    #[test]
    fn test_firmware_log_default() {
        // By default, firmware log should be enabled (pty/console mode)
        let xml = DomainBuilder::new()
            .with_name("test-firmware-log-default")
            .build_xml()
            .unwrap();

        // On x86_64, should have isa-debugcon serial device
        if std::env::consts::ARCH == "x86_64" {
            assert!(xml.contains("serial type=\"pty\""));
            assert!(xml.contains("target type=\"isa-debug\""));
            assert!(xml.contains("model name=\"isa-debugcon\""));
            assert!(xml.contains("address type=\"isa\" iobase=\"0x402\""));
        }
    }

    #[test]
    fn test_firmware_log_file() {
        // Test firmware log to file
        let xml = DomainBuilder::new()
            .with_name("test-firmware-log-file")
            .with_firmware_log(FirmwareLogOutput::File("/tmp/ovmf-debug.log".to_string()))
            .build_xml()
            .unwrap();

        // On x86_64, should have isa-debugcon with file output
        if std::env::consts::ARCH == "x86_64" {
            assert!(xml.contains("serial type=\"file\""));
            assert!(xml.contains("source path=\"/tmp/ovmf-debug.log\""));
            assert!(xml.contains("target type=\"isa-debug\""));
            assert!(xml.contains("model name=\"isa-debugcon\""));
            assert!(xml.contains("address type=\"isa\" iobase=\"0x402\""));
        }
    }

    #[test]
    fn test_firmware_log_disabled() {
        // Test disabling firmware log by setting firmware_log to None after construction
        // Note: There's no public API to disable it once set, but we can test the XML
        // generation doesn't include it on non-x86 architectures
        let xml = DomainBuilder::new()
            .with_name("test-firmware-log")
            .build_xml()
            .unwrap();

        // On non-x86_64, should NOT have isa-debugcon (it's x86-only)
        if std::env::consts::ARCH != "x86_64" {
            assert!(!xml.contains("isa-debugcon"));
            assert!(!xml.contains("isa-debug"));
        }
    }
}
