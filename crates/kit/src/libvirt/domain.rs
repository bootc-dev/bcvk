//! Domain XML generation and management utilities
//!
//! This module provides utilities for generating libvirt domain XML configurations
//! for bootc containers, inspired by the podman-bootc domain builder pattern.

use crate::arch::ArchConfig;
use crate::common_opts::DEFAULT_MEMORY_USER_STR;
use crate::run_ephemeral::default_vcpus;
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
    /// TODO: change libvirt to detect this
    #[allow(dead_code)]
    pub readonly: bool,
}

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
    virtiofs_filesystems: Vec<VirtiofsFilesystem>,
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
            virtiofs_filesystems: Vec::new(),
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

    /// Add a virtiofs filesystem mount
    pub fn with_virtiofs_filesystem(mut self, filesystem: VirtiofsFilesystem) -> Self {
        self.virtiofs_filesystems.push(filesystem);
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

        let mut xml = if self.qemu_args.is_empty() {
            format!(
                r#"<domain type="kvm">
  <name>{}</name>
  <uuid>{}</uuid>
  <memory unit="MiB">{}</memory>
  <currentMemory unit="MiB">{}</currentMemory>
  <vcpu>{}</vcpu>
  <os>
    <type arch="{}" machine="{}">{}</type>
    <boot dev="hd"/>"#,
                name,
                uuid,
                memory,
                memory,
                vcpus,
                arch_config.arch,
                arch_config.machine,
                arch_config.os_type
            )
        } else {
            format!(
                r#"<domain type="kvm" xmlns:qemu="http://libvirt.org/schemas/domain/qemu/1.0">
  <name>{}</name>
  <uuid>{}</uuid>
  <memory unit="MiB">{}</memory>
  <currentMemory unit="MiB">{}</currentMemory>
  <vcpu>{}</vcpu>
  <os>
    <type arch="{}" machine="{}">{}</type>
    <boot dev="hd"/>"#,
                name,
                uuid,
                memory,
                memory,
                vcpus,
                arch_config.arch,
                arch_config.machine,
                arch_config.os_type
            )
        };

        // Add kernel arguments if specified (for direct boot)
        if let Some(ref kargs) = self.kernel_args {
            xml.push_str(&format!("\n    <cmdline>{}</cmdline>", kargs));
        }

        xml.push_str("\n  </os>");

        // Add memory backing for shared memory support (required for virtiofs)
        xml.push_str(
            r#"
  <memoryBacking>
    <source type="memfd"/>
    <access mode="shared"/>
  </memoryBacking>"#,
        );

        // Architecture-specific features
        xml.push_str(arch_config.xml_features());

        // Architecture-specific CPU configuration
        xml.push_str(&format!(
            r#"
  <cpu mode="{}"/>"#,
            arch_config.cpu_mode()
        ));

        // Clock and lifecycle configuration
        xml.push_str(
            r#"
  <clock offset="utc">"#,
        );

        // Architecture-specific timers
        xml.push_str(arch_config.xml_timers());

        xml.push_str(
            r#"
  </clock>
  <on_poweroff>destroy</on_poweroff>
  <on_reboot>restart</on_reboot>
  <on_crash>destroy</on_crash>"#,
        );

        // Devices section
        xml.push_str("\n  <devices>");

        // Let libvirt automatically detect the appropriate emulator
        // based on the architecture and domain type

        // Disk
        if let Some(ref disk_path) = self.disk_path {
            xml.push_str(&format!(
                r#"
    <disk type="file" device="disk">
      <driver name="qemu" type="raw"/>
      <source file="{}"/>
      <target dev="vda" bus="virtio"/>
    </disk>"#,
                disk_path
            ));
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
                xml.push_str(
                    r#"
    <interface type="user">
      <model type="virtio"/>
    </interface>"#,
                );
            }
            network if network.starts_with("bridge=") => {
                let bridge_name = &network[7..]; // Remove "bridge=" prefix
                xml.push_str(&format!(
                    r#"
    <interface type="bridge">
      <source bridge="{}"/>
      <model type="virtio"/>
    </interface>"#,
                    bridge_name
                ));
            }
            _ => {
                // Assume it's a network name
                xml.push_str(&format!(
                    r#"
    <interface type="network">
      <source network="{}"/>
      <model type="virtio"/>
    </interface>"#,
                    network_config
                ));
            }
        }

        // Serial console
        xml.push_str(
            r#"
    <serial type="pty">
      <target port="0"/>
    </serial>
    <console type="pty">
      <target type="serial" port="0"/>
    </console>"#,
        );

        // VNC graphics if enabled
        if let Some(vnc_port) = self.vnc_port {
            xml.push_str(&format!(
                r#"
    <graphics type="vnc" port="{}" listen="127.0.0.1"/>
    <video>
      <model type="vga"/>
    </video>"#,
                vnc_port
            ));
        }

        // Virtiofs filesystems
        for filesystem in &self.virtiofs_filesystems {
            xml.push_str(&format!(
                r#"
    <filesystem type="mount" accessmode="passthrough">
      <driver type="virtiofs" queue="1024"/>
      <source dir="{}"/>
      <target dir="{}"/>"#,
                filesystem.source_dir, filesystem.tag
            ));
            // TODO: detect libvirt version for this
            //         if filesystem.readonly {
            //             xml.push_str(
            //                 r#"
            //   <readonly/>"#,
            //             );
            //         }
            xml.push_str(
                r#"
    </filesystem>"#,
            );
        }

        xml.push_str("\n  </devices>");

        // QEMU commandline section (if we have QEMU args)
        if !self.qemu_args.is_empty() {
            xml.push_str("\n  <qemu:commandline>");
            for arg in &self.qemu_args {
                xml.push_str(&format!("\n    <qemu:arg value='{}'/>", arg));
            }
            xml.push_str("\n  </qemu:commandline>");
        }

        // Metadata section
        if !self.metadata.is_empty() {
            xml.push_str("\n  <metadata>");
            xml.push_str(
                "\n    <bootc:container xmlns:bootc=\"https://github.com/containers/bootc\">",
            );

            for (key, value) in &self.metadata {
                // Strip bootc: prefix if present for cleaner XML
                let clean_key = key.strip_prefix("bootc:").unwrap_or(key);
                xml.push_str(&format!(
                    "\n      <bootc:{}>{}</bootc:{}>",
                    clean_key, value, clean_key
                ));
            }

            xml.push_str("\n    </bootc:container>");
            xml.push_str("\n  </metadata>");
        }

        xml.push_str("\n</domain>");

        Ok(xml)
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
                assert!(xml.contains("vmport state='off'")); // x86_64-specific feature
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
        assert!(xml.contains("<timer name='rtc'"));
    }
}
