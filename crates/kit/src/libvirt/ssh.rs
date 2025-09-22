//! SSH to libvirt domains with embedded SSH credentials
//!
//! This module provides functionality to SSH to libvirt domains that were created
//! with SSH key injection, automatically retrieving SSH credentials from domain XML
//! metadata and establishing connection using embedded private keys.

use crate::xml_utils;
use base64::Engine;
use clap::Parser;
use color_eyre::{eyre::eyre, Result};
use std::fs::Permissions;
use std::io::Write;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::CommandExt;
use std::process::Command;
use tempfile;
use tracing::debug;

/// Configuration options for SSH connection to libvirt domain
#[derive(Debug, Parser)]
pub struct LibvirtSshOpts {
    /// Name of the libvirt domain to connect to
    pub domain_name: String,

    /// Hypervisor connection URI (e.g., qemu:///system, qemu+ssh://host/system)
    #[clap(short = 'c', long = "connect")]
    pub connect: Option<String>,

    /// SSH username to use for connection (defaults to 'root')
    #[clap(long, default_value = "root")]
    pub user: String,

    /// Command to execute on remote host
    pub command: Vec<String>,

    /// Use strict host key checking
    #[clap(long)]
    pub strict_host_keys: bool,

    /// SSH connection timeout in seconds
    #[clap(long, default_value = "30")]
    pub timeout: u32,
}

/// SSH configuration extracted from domain metadata
#[derive(Debug)]
struct DomainSshConfig {
    private_key_content: String,
    ssh_port: u16,
    is_generated: bool,
}

impl LibvirtSshOpts {
    /// Build a virsh command with optional connection URI
    fn virsh_command(&self) -> Command {
        let mut cmd = Command::new("virsh");
        if let Some(ref connect) = self.connect {
            cmd.arg("-c").arg(connect);
        }
        cmd
    }

    /// Check if domain exists and is accessible
    fn check_domain_exists(&self) -> Result<bool> {
        let output = self
            .virsh_command()
            .args(&["dominfo", &self.domain_name])
            .output()?;

        Ok(output.status.success())
    }

    /// Get domain state
    fn get_domain_state(&self) -> Result<String> {
        let output = self
            .virsh_command()
            .args(&["domstate", &self.domain_name])
            .output()?;

        if output.status.success() {
            let state = String::from_utf8(output.stdout)?;
            Ok(state.trim().to_string())
        } else {
            Err(eyre!("Failed to get domain state"))
        }
    }

    /// Extract SSH configuration from domain XML metadata
    fn extract_ssh_config(&self) -> Result<DomainSshConfig> {
        let output = self
            .virsh_command()
            .args(&["dumpxml", &self.domain_name])
            .output()?;

        if !output.status.success() {
            return Err(eyre!("Failed to get domain XML for '{}'", self.domain_name));
        }

        let xml = String::from_utf8(output.stdout)?;
        debug!("Domain XML for SSH extraction: {}", xml);

        // Parse XML once for all metadata extraction
        let dom = xml_utils::parse_xml_dom(&xml)
            .map_err(|e| eyre!("Failed to parse domain XML: {}", e))?;

        // Extract SSH metadata from bootc:container section
        // First try the new base64 encoded format
        let private_key = if let Some(encoded_key_node) =
            dom.find_with_namespace("ssh-private-key-base64")
        {
            let encoded_key = encoded_key_node.text_content();
            debug!("Found base64 encoded SSH private key");
            // Decode base64 encoded private key
            let decoded_bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded_key)
                .map_err(|e| eyre!("Failed to decode base64 SSH private key: {}", e))?;

            String::from_utf8(decoded_bytes)
                .map_err(|e| eyre!("SSH private key contains invalid UTF-8: {}", e))?
        } else if let Some(legacy_key_node) = dom.find_with_namespace("ssh-private-key") {
            debug!("Found legacy plain text SSH private key");
            legacy_key_node.text_content().to_string()
        } else {
            return Err(eyre!("No SSH private key found in domain '{}' metadata. Domain was not created with --generate-ssh-key or --ssh-key.", self.domain_name));
        };

        // Debug: Verify SSH key format
        debug!(
            "Extracted SSH private key length: {} bytes",
            private_key.len()
        );
        debug!(
            "SSH key starts with: {}",
            if private_key.len() > 50 {
                &private_key[..50]
            } else {
                &private_key
            }
        );

        // Validate SSH key format
        if !private_key.contains("BEGIN") || !private_key.contains("PRIVATE KEY") {
            return Err(eyre!(
                "Invalid SSH private key format in domain metadata. Expected OpenSSH private key."
            ));
        }

        // Ensure the key has proper line endings - SSH keys are sensitive to this
        let private_key = private_key.replace("\r\n", "\n").replace("\r", "\n");

        // Ensure key ends with exactly one newline
        let private_key = private_key.trim_end().to_string() + "\n";

        debug!(
            "SSH private key after normalization: {} chars, ends with newline: {}",
            private_key.len(),
            private_key.ends_with('\n')
        );

        // Verify key structure more thoroughly
        let lines: Vec<&str> = private_key.lines().collect();
        debug!("SSH key has {} lines", lines.len());
        if lines.is_empty() {
            return Err(eyre!("SSH private key is empty after line normalization"));
        }
        if !lines[0].trim().starts_with("-----BEGIN") {
            return Err(eyre!(
                "SSH private key first line malformed: '{}'",
                lines[0]
            ));
        }
        if !lines.last().unwrap().trim().starts_with("-----END") {
            return Err(eyre!(
                "SSH private key last line malformed: '{}'",
                lines.last().unwrap()
            ));
        }

        let ssh_port_str = dom.find_with_namespace("ssh-port").ok_or_else(|| {
            eyre!(
                "No SSH port found in domain '{}' metadata",
                self.domain_name
            )
        })?;

        let ssh_port = ssh_port_str
            .text_content()
            .parse::<u16>()
            .map_err(|e| eyre!("Invalid SSH port '{}': {}", ssh_port_str.text_content(), e))?;

        let is_generated = dom
            .find_with_namespace("ssh-generated")
            .map(|node| node.text_content() == "true")
            .unwrap_or(false);

        Ok(DomainSshConfig {
            private_key_content: private_key,
            ssh_port,
            is_generated,
        })
    }

    /// Create temporary SSH private key file and return its path
    fn create_temp_ssh_key(&self, ssh_config: &DomainSshConfig) -> Result<tempfile::NamedTempFile> {
        debug!(
            "Creating temporary SSH key file with {} bytes",
            ssh_config.private_key_content.len()
        );

        let mut temp_key = tempfile::NamedTempFile::new()
            .map_err(|e| eyre!("Failed to create temporary SSH key file: {}", e))?;

        debug!("Temporary SSH key file created at: {:?}", temp_key.path());

        // Write the key content first
        temp_key.write_all(ssh_config.private_key_content.as_bytes())?;
        temp_key.flush()?;

        // Set strict permissions (user read/write only)
        let perms = Permissions::from_mode(0o600);
        temp_key
            .as_file()
            .set_permissions(perms)
            .map_err(|e| eyre!("Failed to set SSH key file permissions: {}", e))?;

        debug!("SSH key file permissions set to 0o600");

        // Verify the file is readable and has correct content
        let written_content = std::fs::read_to_string(temp_key.path())
            .map_err(|e| eyre!("Failed to verify written SSH key file: {}", e))?;

        if written_content != ssh_config.private_key_content {
            return Err(eyre!("SSH key file content verification failed"));
        }

        debug!("SSH key file verification successful");

        Ok(temp_key)
    }

    /// Execute SSH connection to domain
    fn connect_ssh(&self, ssh_config: &DomainSshConfig) -> Result<()> {
        debug!(
            "Connecting to domain '{}' via SSH on port {} (user: {})",
            self.domain_name, ssh_config.ssh_port, self.user
        );

        if ssh_config.is_generated {
            debug!("Using ephemeral SSH key from domain metadata");
        }

        // Create temporary SSH key file
        let temp_key = self.create_temp_ssh_key(ssh_config)?;

        // Build SSH command
        let mut ssh_cmd = Command::new("ssh");

        // Basic SSH options
        ssh_cmd
            .arg("-i")
            .arg(temp_key.path())
            .arg("-p")
            .arg(ssh_config.ssh_port.to_string())
            .args(["-o", "IdentitiesOnly=yes"])
            .arg("-o")
            .arg("PasswordAuthentication=no")
            .arg("-o")
            .arg("ConnectTimeout=30")
            .arg("-o")
            .arg("ServerAliveInterval=60");

        // Host key checking
        if !self.strict_host_keys {
            ssh_cmd
                .arg("-o")
                .arg("StrictHostKeyChecking=no")
                .arg("-o")
                .arg("UserKnownHostsFile=/dev/null");
        }

        // Target host
        ssh_cmd.arg(format!("{}@127.0.0.1", self.user));

        // Add command if specified
        if !self.command.is_empty() {
            ssh_cmd.arg("--");
            ssh_cmd.args(&self.command);
        }

        debug!("Executing SSH command: {:?}", ssh_cmd);

        // For commands (non-interactive SSH), capture output
        // For interactive SSH (no command), exec to replace current process
        if self.command.is_empty() {
            // Interactive SSH - exec to replace the current process
            // This provides the cleanest terminal experience
            debug!("Executing interactive SSH session via exec");

            let error = ssh_cmd.exec();
            // exec() only returns on error
            return Err(eyre!("Failed to exec SSH command: {}", error));
        } else {
            // Command execution - capture and forward output
            let output = ssh_cmd
                .output()
                .map_err(|e| eyre!("Failed to execute SSH command: {}", e))?;

            if !output.stdout.is_empty() {
                // Forward stdout to parent process
                print!("{}", String::from_utf8_lossy(&output.stdout));
                debug!("SSH stdout: {}", String::from_utf8_lossy(&output.stdout));
            }
            if !output.stderr.is_empty() {
                // Forward stderr to parent process
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
                debug!("SSH stderr: {}", String::from_utf8_lossy(&output.stderr));
            }

            if !output.status.success() {
                return Err(eyre!(
                    "SSH connection failed with exit code: {}",
                    output.status.code().unwrap_or(-1)
                ));
            }
        }

        Ok(())
    }
}

/// Execute the libvirt SSH command
pub fn run(opts: LibvirtSshOpts) -> Result<()> {
    debug!("Connecting to libvirt domain: {}", opts.domain_name);

    // Check if domain exists
    if !opts.check_domain_exists()? {
        return Err(eyre!("Domain '{}' not found", opts.domain_name));
    }

    // Check if domain is running
    let state = opts.get_domain_state()?;
    if state != "running" {
        return Err(eyre!(
            "Domain '{}' is not running (current state: {}). Start it first with: virsh start {}",
            opts.domain_name,
            state,
            opts.domain_name
        ));
    }

    // Extract SSH configuration from domain metadata
    let ssh_config = opts.extract_ssh_config()?;

    // Connect via SSH
    opts.connect_ssh(&ssh_config)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ssh_metadata_extraction() {
        let xml = r#"
<domain>
  <metadata>
    <bootc:container xmlns:bootc="https://github.com/containers/bootc">
      <bootc:ssh-private-key>-----BEGIN OPENSSH PRIVATE KEY-----</bootc:ssh-private-key>
      <bootc:ssh-port>2222</bootc:ssh-port>
      <bootc:ssh-generated>true</bootc:ssh-generated>
    </bootc:container>
  </metadata>
</domain>
        "#;

        let dom = xml_utils::parse_xml_dom(xml).unwrap();

        assert_eq!(
            dom.find_with_namespace("ssh-private-key")
                .map(|n| n.text_content().to_string()),
            Some("-----BEGIN OPENSSH PRIVATE KEY-----".to_string())
        );

        assert_eq!(
            dom.find_with_namespace("ssh-port")
                .map(|n| n.text_content().to_string()),
            Some("2222".to_string())
        );

        assert_eq!(
            dom.find_with_namespace("ssh-generated")
                .map(|n| n.text_content().to_string()),
            Some("true".to_string())
        );

        assert_eq!(
            dom.find_with_namespace("nonexistent")
                .map(|n| n.text_content().to_string()),
            None
        );
    }
}
