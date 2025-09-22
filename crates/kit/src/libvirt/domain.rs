//! Domain XML generation and management utilities
//!
//! This module provides utilities for generating libvirt domain XML configurations
//! for bootc containers, inspired by the podman-bootc domain builder pattern.

use crate::arch::ArchConfig;
use crate::common_opts::DEFAULT_MEMORY_USER_STR;
use crate::run_ephemeral::default_vcpus;
use crate::xml_utils::XmlWriter;
use color_eyre::{eyre::eyre, Result};
use std::collections::HashMap;
use uuid::Uuid;

/// Builder for creating libvirt domain XML configurations
#[derive(Debug)]
pub struct DomainBuilder {
    name: Option<String>,
    uuid: Option<String>,
    memory: Option<u64>, // in MB
    vcpus: Option<u32>,
    disk_path: Option<String>,
    network: Option<String>,
    vnc_port: Option<u16>,
    kernel_args: Option<String>,
    metadata: HashMap<String, String>,
    qemu_args: Vec<String>,
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
            network: None,
            vnc_port: None,
            kernel_args: None,
            metadata: HashMap::new(),
            qemu_args: Vec::new(),
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

    /// Set network configuration
    pub fn with_network(mut self, network: &str) -> Self {
        self.network = Some(network.to_string());
        self
    }

    /// Enable VNC on specified port
    pub fn with_vnc(mut self, port: u16) -> Self {
        self.vnc_port = Some(port);
        self
    }

    /// Set kernel arguments for direct boot
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

        // OS section
        writer.start_element("os", &[])?;
        writer.write_text_element_with_attrs(
            "type",
            &arch_config.os_type,
            &[
                ("arch", &arch_config.arch),
                ("machine", &arch_config.machine),
            ],
        )?;
        writer.write_empty_element("boot", &[("dev", "hd")])?;

        // Add kernel arguments if specified (for direct boot)
        if let Some(ref kargs) = self.kernel_args {
            writer.write_text_element("cmdline", kargs)?;
        }

        writer.end_element("os")?;

        // Architecture-specific features
        arch_config.write_features(&mut writer)?;

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
            writer.start_element("disk", &[("type", "file"), ("device", "disk")])?;
            writer.write_empty_element("driver", &[("name", "qemu"), ("type", "raw")])?;
            writer.write_empty_element("source", &[("file", disk_path)])?;
            writer.write_empty_element("target", &[("dev", "vda"), ("bus", "virtio")])?;
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
                let bridge_name = &network[7..]; // Remove "bridge=" prefix
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

        // Serial console
        writer.start_element("serial", &[("type", "pty")])?;
        writer.write_empty_element("target", &[("port", "0")])?;
        writer.end_element("serial")?;

        writer.start_element("console", &[("type", "pty")])?;
        writer.write_empty_element("target", &[("type", "serial"), ("port", "0")])?;
        writer.end_element("console")?;

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
}
