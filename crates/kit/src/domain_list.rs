//! Domain listing utilities for bootc VMs
//!
//! This module provides functionality to list libvirt domains created by bcvk libvirt,
//! using libvirt as the source of truth instead of the VmRegistry cache.

use crate::xml_utils;
use color_eyre::{eyre::Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::SystemTime;

/// Information about a podman-bootc domain from libvirt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodmanBootcDomain {
    /// Domain name
    pub name: String,
    /// Domain state (running, shut off, etc.)
    pub state: String,
    /// Container image used to create the domain
    pub image: Option<String>,
    /// Domain creation timestamp (if available)
    pub created: Option<SystemTime>,
    /// Memory allocation in MB
    pub memory_mb: Option<u32>,
    /// Number of virtual CPUs
    pub vcpus: Option<u32>,
    /// Disk path
    pub disk_path: Option<String>,
    /// User-defined labels for organizing domains
    pub labels: Vec<String>,
}

impl PodmanBootcDomain {
    /// Check if this domain is running
    pub fn is_running(&self) -> bool {
        self.state == "running"
    }

    /// Check if this domain is stopped
    #[allow(dead_code)]
    pub fn is_stopped(&self) -> bool {
        self.state == "shut off"
    }

    /// Get status as string for display
    pub fn status_string(&self) -> String {
        match self.state.as_str() {
            "running" => "running".to_string(),
            "shut off" => "stopped".to_string(),
            "paused" => "paused".to_string(),
            other => other.to_string(),
        }
    }
}

/// Domain listing manager
pub struct DomainLister {
    /// Optional libvirt connection URI
    pub connect_uri: Option<String>,
}

impl Default for DomainLister {
    fn default() -> Self {
        Self::new()
    }
}

impl DomainLister {
    /// Create a new domain lister
    pub fn new() -> Self {
        Self { connect_uri: None }
    }

    /// Create a domain lister with custom connection URI
    #[allow(dead_code)]
    pub fn with_connection(connect_uri: String) -> Self {
        Self {
            connect_uri: Some(connect_uri),
        }
    }

    /// Build a virsh command with optional connection URI
    fn virsh_command(&self) -> Command {
        let mut cmd = Command::new("virsh");
        if let Some(ref uri) = self.connect_uri {
            cmd.arg("-c").arg(uri);
        }
        cmd
    }

    /// List all domains (running and inactive)
    pub fn list_all_domains(&self) -> Result<Vec<String>> {
        let output = self
            .virsh_command()
            .args(&["list", "--all", "--name"])
            .output()
            .with_context(|| "Failed to run virsh list")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(color_eyre::eyre::eyre!(
                "Failed to list domains: {}",
                stderr
            ));
        }

        let domain_names = String::from_utf8(output.stdout)?
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect();

        Ok(domain_names)
    }

    /// Get domain state information
    pub fn get_domain_state(&self, domain_name: &str) -> Result<String> {
        let output = self
            .virsh_command()
            .args(&["domstate", domain_name])
            .output()
            .with_context(|| format!("Failed to get state for domain '{}'", domain_name))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(color_eyre::eyre::eyre!(
                "Failed to get domain state for '{}': {}",
                domain_name,
                stderr
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    /// Get domain XML metadata as parsed DOM
    pub fn get_domain_xml(&self, domain_name: &str) -> Result<xml_utils::XmlNode> {
        crate::libvirt::run::run_virsh_xml(self.connect_uri.as_deref(), &["dumpxml", domain_name])
            .context(format!("Failed to get XML for domain '{}'", domain_name))
    }

    /// Extract podman-bootc metadata from parsed domain XML
    fn extract_podman_bootc_metadata(
        &self,
        dom: &xml_utils::XmlNode,
    ) -> Result<Option<PodmanBootcDomainMetadata>> {
        // Look for bootc metadata in the XML
        // This could be in various forms:
        // 1. <bootc:source-image> in metadata section
        // 2. Domain name pattern (created by bcvk libvirt)
        // 3. Domain description containing bcvk signature

        // Try to extract source image from bootc metadata
        let source_image = dom
            .find("bootc:source-image")
            .or_else(|| dom.find("source-image"))
            .map(|node| node.text_content().to_string());

        // Extract other metadata
        let created = dom
            .find("bootc:created")
            .or_else(|| dom.find("created"))
            .map(|node| node.text_content().to_string());

        // Extract labels (comma-separated)
        let labels = dom
            .find("bootc:label")
            .or_else(|| dom.find("label"))
            .map(|node| {
                node.text_content()
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        // Extract memory and vcpu from domain XML
        let memory_mb = dom.find("memory").and_then(|node| {
            // Memory might have unit attribute, but we'll try to parse the value
            node.text_content().parse::<u32>().ok()
        });

        let vcpus = dom
            .find("vcpu")
            .and_then(|node| node.text_content().parse::<u32>().ok());

        // Extract disk path from first disk device
        let disk_path = extract_disk_path(&dom);

        Ok(Some(PodmanBootcDomainMetadata {
            source_image,
            created,
            memory_mb,
            vcpus,
            disk_path,
            labels,
        }))
    }

    /// Check if a domain was created by bcvk libvirt
    fn is_podman_bootc_domain(&self, _domain_name: &str, dom: &xml_utils::XmlNode) -> bool {
        // Only use XML metadata - domains created by bcvk libvirt should have bootc metadata
        dom.find("bootc:source-image").is_some()
            || dom.find("source-image").is_some()
            || dom.find("bootc:container").is_some()
    }

    /// Get detailed information about a domain with pre-parsed XML
    pub fn get_domain_info_from_xml(
        &self,
        domain_name: &str,
        dom: &xml_utils::XmlNode,
    ) -> Result<PodmanBootcDomain> {
        let state = self.get_domain_state(domain_name)?;
        let metadata = self.extract_podman_bootc_metadata(dom)?;

        Ok(PodmanBootcDomain {
            name: domain_name.to_string(),
            state,
            image: metadata.as_ref().and_then(|m| m.source_image.clone()),
            created: None, // TODO: Parse created timestamp
            memory_mb: metadata.as_ref().and_then(|m| m.memory_mb),
            vcpus: metadata.as_ref().and_then(|m| m.vcpus),
            disk_path: metadata.as_ref().and_then(|m| m.disk_path.clone()),
            labels: metadata
                .as_ref()
                .map(|m| m.labels.clone())
                .unwrap_or_default(),
        })
    }

    /// Get detailed information about a domain
    pub fn get_domain_info(&self, domain_name: &str) -> Result<PodmanBootcDomain> {
        let dom = self.get_domain_xml(domain_name)?;
        self.get_domain_info_from_xml(domain_name, &dom)
    }

    /// List all bootc domains
    pub fn list_bootc_domains(&self) -> Result<Vec<PodmanBootcDomain>> {
        let all_domains = self.list_all_domains()?;
        let mut podman_bootc_domains = Vec::new();

        for domain_name in all_domains {
            // Get domain XML to check if it's a podman-bootc domain
            match self.get_domain_xml(&domain_name) {
                Ok(dom) => {
                    if self.is_podman_bootc_domain(&domain_name, &dom) {
                        // Use the already-parsed DOM instead of re-parsing
                        match self.get_domain_info_from_xml(&domain_name, &dom) {
                            Ok(domain_info) => podman_bootc_domains.push(domain_info),
                            Err(e) => {
                                eprintln!(
                                    "Warning: Failed to get info for domain '{}': {}",
                                    domain_name, e
                                );
                                // Continue with other domains
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to get XML for domain '{}': {}",
                        domain_name, e
                    );
                    // Continue with other domains
                }
            }
        }

        Ok(podman_bootc_domains)
    }

    /// List running bootc domains only
    pub fn list_running_bootc_domains(&self) -> Result<Vec<PodmanBootcDomain>> {
        let all_domains = self.list_bootc_domains()?;
        Ok(all_domains.into_iter().filter(|d| d.is_running()).collect())
    }
}

/// Internal structure for extracting metadata
#[derive(Debug)]
struct PodmanBootcDomainMetadata {
    source_image: Option<String>,
    #[allow(dead_code)]
    created: Option<String>,
    memory_mb: Option<u32>,
    vcpus: Option<u32>,
    disk_path: Option<String>,
    labels: Vec<String>,
}

/// Extract disk path from domain XML using DOM parser
fn extract_disk_path(dom: &xml_utils::XmlNode) -> Option<String> {
    // Look for first disk device with type="file"
    // We need to find: <disk type="file"><source file="/path/to/disk"/></disk>
    find_disk_with_file_type(dom)
        .and_then(|disk_node| disk_node.find("source"))
        .and_then(|source_node| source_node.attributes.get("file"))
        .map(|path| path.clone())
}

/// Recursively find a disk element with type="file"
fn find_disk_with_file_type(node: &xml_utils::XmlNode) -> Option<&xml_utils::XmlNode> {
    if node.name == "disk" {
        if let Some(disk_type) = node.attributes.get("type") {
            if disk_type == "file" {
                return Some(node);
            }
        }
    }

    for child in &node.children {
        if let Some(found) = find_disk_with_file_type(child) {
            return Some(found);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml_utils;

    #[test]
    fn test_dom_xml_parsing() {
        let xml = r#"
        <domain>
            <memory unit='MiB'>2048</memory>
            <vcpu>4</vcpu>
            <metadata>
                <bootc:source-image>quay.io/fedora/fedora-bootc:40</bootc:source-image>
            </metadata>
        </domain>
        "#;

        let dom = xml_utils::parse_xml_dom(xml).unwrap();
        assert_eq!(
            dom.find("memory").map(|n| n.text_content().to_string()),
            Some("2048".to_string())
        );
        assert_eq!(
            dom.find("vcpu").map(|n| n.text_content().to_string()),
            Some("4".to_string())
        );
        assert_eq!(
            dom.find("bootc:source-image")
                .map(|n| n.text_content().to_string()),
            Some("quay.io/fedora/fedora-bootc:40".to_string())
        );
        assert_eq!(
            dom.find("nonexistent")
                .map(|n| n.text_content().to_string()),
            None
        );
    }

    #[test]
    fn test_extract_disk_path() {
        let xml = r#"
        <domain>
            <devices>
                <disk type="file" device="disk">
                    <driver name="qemu" type="raw"/>
                    <source file="/var/lib/libvirt/images/test.raw"/>
                    <target dev="vda" bus="virtio"/>
                </disk>
            </devices>
        </domain>
        "#;

        let dom = xml_utils::parse_xml_dom(xml).unwrap();
        assert_eq!(
            extract_disk_path(&dom),
            Some("/var/lib/libvirt/images/test.raw".to_string())
        );
    }

    #[test]
    fn test_domain_status_mapping() {
        let domain = PodmanBootcDomain {
            name: "test".to_string(),
            state: "running".to_string(),
            image: None,
            created: None,
            memory_mb: None,
            vcpus: None,
            disk_path: None,
            labels: vec![],
        };

        assert!(domain.is_running());
        assert!(!domain.is_stopped());
        assert_eq!(domain.status_string(), "running");

        let stopped_domain = PodmanBootcDomain {
            name: "test".to_string(),
            state: "shut off".to_string(),
            image: None,
            created: None,
            memory_mb: None,
            vcpus: None,
            disk_path: None,
            labels: vec![],
        };

        assert!(!stopped_domain.is_running());
        assert!(stopped_domain.is_stopped());
        assert_eq!(stopped_domain.status_string(), "stopped");
    }
}
