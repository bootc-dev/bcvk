//! SSH to libvirt domains with embedded SSH credentials
//!
//! This module provides functionality to SSH to libvirt domains that were created
//! with SSH key injection, automatically retrieving SSH credentials from domain XML
//! metadata and establishing connection using embedded private keys.

use base64::Engine;
use clap::Parser;
use color_eyre::{
    eyre::{eyre, Context},
    Result,
};
use std::fs::Permissions;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile;
use tracing::debug;

/// Configuration options for SSH connection to libvirt domain
#[derive(Debug, Parser)]
pub struct LibvirtSshOpts {
    /// Name of the libvirt domain to connect to
    pub domain_name: String,

    /// SSH username to use for connection (defaults to 'root')
    #[clap(long, default_value = "root")]
    pub user: String,

    /// Command to execute on remote host
    pub command: Vec<String>,

    /// Use strict host key checking
    #[clap(long)]
    pub strict_host_keys: bool,

    /// SSH connection timeout in seconds
    #[clap(long, default_value = "5")]
    pub timeout: u32,

    /// SSH log level
    #[clap(long, default_value = "ERROR")]
    pub log_level: String,

    /// Extra SSH options in key=value format
    #[clap(long)]
    pub extra_options: Vec<String>,

    /// Suppress stdout/stderr output (for connectivity testing)
    #[clap(skip)]
    pub suppress_output: bool,
}

/// SSH configuration extracted from domain metadata
#[derive(Debug)]
struct DomainSshConfig {
    private_key_content: String,
    ssh_port: u16,
    is_generated: bool,
}

impl LibvirtSshOpts {
    /// Check if domain exists and is accessible
    fn check_domain_exists(&self, global_opts: &crate::libvirt::LibvirtOptions) -> Result<bool> {
        let output = global_opts
            .virsh_command()
            .args(&["dominfo", &self.domain_name])
            .output()?;

        Ok(output.status.success())
    }

    /// Get domain state
    fn get_domain_state(&self, global_opts: &crate::libvirt::LibvirtOptions) -> Result<String> {
        let output = global_opts
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
    fn extract_ssh_config(
        &self,
        global_opts: &crate::libvirt::LibvirtOptions,
    ) -> Result<DomainSshConfig> {
        let dom = super::run::run_virsh_xml(
            global_opts.connect.as_deref(),
            &["dumpxml", &self.domain_name],
        )
        .context(format!(
            "Failed to get domain XML for '{}'",
            self.domain_name
        ))?;
        debug!("Domain XML retrieved for SSH extraction");

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

    /// Build SSH command with configured options
    fn build_ssh_command(
        &self,
        ssh_config: &DomainSshConfig,
        temp_key: &tempfile::NamedTempFile,
        parsed_extra_options: Vec<(String, String)>,
    ) -> Command {
        let mut ssh_cmd = Command::new("ssh");
        ssh_cmd
            .arg("-i")
            .arg(temp_key.path())
            .arg("-p")
            .arg(ssh_config.ssh_port.to_string());

        let common_opts = crate::ssh::CommonSshOptions {
            strict_host_keys: self.strict_host_keys,
            connect_timeout: self.timeout,
            server_alive_interval: 60,
            log_level: self.log_level.clone(),
            extra_options: parsed_extra_options,
        };
        common_opts.apply_to_command(&mut ssh_cmd);
        ssh_cmd.arg(format!("{}@127.0.0.1", self.user));

        ssh_cmd
    }

    /// Show recent console output from domain
    fn show_console_feedback(&self, global_opts: &crate::libvirt::LibvirtOptions) -> Result<()> {
        debug!("Fetching console output for feedback");

        let mut cmd = global_opts.virsh_command();
        cmd.args(&["console", "--force", &self.domain_name]);

        let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;

        let mut lines_shown = 0;
        const MAX_LINES: usize = 5;

        if let Some(stdout) = child.stdout.take() {
            // Spawn a thread to read console output
            let suppress_output = self.suppress_output;
            let handle = std::thread::spawn(move || {
                let reader = BufReader::new(stdout);
                let mut lines = Vec::new();

                for line in reader.lines() {
                    if let Ok(line) = line {
                        // Only collect interesting lines
                        if line.contains("Reached target")
                            || line.contains("Started")
                            || line.contains("ssh")
                            || line.contains("sshd")
                            || line.contains("login:")
                        {
                            lines.push(line);
                            if lines.len() >= MAX_LINES {
                                break;
                            }
                        }
                    }
                }
                (lines, suppress_output)
            });

            // Give the thread a moment to read available output
            std::thread::sleep(Duration::from_millis(500));

            // Kill the virsh console process to close the pipe and allow thread to exit
            if let Err(e) = child.kill() {
                debug!("Failed to kill virsh console: {}", e);
            }

            // Now join the thread - it should exit quickly since the pipe is closed
            match handle.join() {
                Ok((lines, suppress)) => {
                    if !lines.is_empty() {
                        for line in lines {
                            if !suppress {
                                eprintln!("  Console: {}", line.trim());
                            }
                            lines_shown += 1;
                        }
                    } else if !suppress {
                        eprintln!("  Console: (no recent output)");
                    }
                }
                Err(_) => {
                    debug!("Console reader thread panicked");
                }
            }
        } else {
            // No stdout, just kill the process
            if let Err(e) = child.kill() {
                debug!("Failed to kill virsh console: {}", e);
            }
        }

        // Wait for child to terminate to avoid zombie processes
        let wait_start = Instant::now();
        while wait_start.elapsed() < Duration::from_millis(500) {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => {
                    debug!("Error waiting for virsh console: {}", e);
                    break;
                }
            }
        }
        // Final wait to reap zombie
        let _ = child.wait();

        debug!("Showed {} console lines", lines_shown);
        Ok(())
    }

    /// Execute SSH connection to domain with retries and feedback
    fn connect_ssh(
        &self,
        global_opts: &crate::libvirt::LibvirtOptions,
        ssh_config: &DomainSshConfig,
    ) -> Result<()> {
        debug!(
            "Connecting to domain '{}' via SSH on port {} (user: {})",
            self.domain_name, ssh_config.ssh_port, self.user
        );

        if ssh_config.is_generated {
            debug!("Using ephemeral SSH key from domain metadata");
        }

        // Create temporary SSH key file
        let temp_key = self.create_temp_ssh_key(ssh_config)?;

        // Parse extra options
        let mut parsed_extra_options = Vec::new();
        for option in &self.extra_options {
            if let Some((key, value)) = option.split_once('=') {
                parsed_extra_options.push((key.to_string(), value.to_string()));
            } else {
                return Err(eyre!(
                    "Invalid extra option format '{}'. Expected 'key=value'",
                    option
                ));
            }
        }

        // For interactive SSH, just exec directly
        if self.command.is_empty() {
            debug!("Executing interactive SSH session via exec");
            let mut ssh_cmd = self.build_ssh_command(ssh_config, &temp_key, parsed_extra_options);
            let error = ssh_cmd.exec();
            return Err(eyre!("Failed to exec SSH command: {}", error));
        }

        // For command execution: retry with console feedback (2 attempts)
        let start_time = Instant::now();

        for attempt in 1..=2 {
            debug!("SSH connection attempt {}/2", attempt);

            // Build SSH command
            let mut ssh_cmd =
                self.build_ssh_command(ssh_config, &temp_key, parsed_extra_options.clone());

            // Add command
            ssh_cmd.arg("--");
            if self.command.len() > 1 {
                let combined_command = crate::ssh::shell_escape_command(&self.command)
                    .map_err(|e| eyre!("Failed to escape shell command: {}", e))?;
                ssh_cmd.arg(combined_command);
            } else {
                ssh_cmd.args(&self.command);
            }

            // Try SSH
            let output = ssh_cmd
                .output()
                .map_err(|e| eyre!("Failed to execute SSH command: {}", e))?;

            if output.status.success() {
                if !output.stdout.is_empty() && !self.suppress_output {
                    print!("{}", String::from_utf8_lossy(&output.stdout));
                }
                debug!(
                    "SSH connected after {:.1}s",
                    start_time.elapsed().as_secs_f64()
                );
                return Ok(());
            }

            // Check if retryable (common errors only)
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            let is_retryable = stderr_str.contains("Connection refused")
                || stderr_str.contains("Connection timed out")
                || stderr_str.contains("banner exchange");

            if !is_retryable || attempt == 2 {
                // Non-retryable or last attempt - fail
                if !self.suppress_output {
                    eprint!("{}", stderr_str);
                }
                return Err(eyre!(
                    "SSH connection failed after {:.1}s",
                    start_time.elapsed().as_secs_f64()
                ));
            }

            // Retryable error - show console feedback
            if !self.suppress_output {
                eprintln!("SSH not ready yet, checking console output...");
            }
            if let Err(e) = self.show_console_feedback(global_opts) {
                debug!("Failed to fetch console output: {}", e);
            }
            std::thread::sleep(Duration::from_secs(2));
        }

        unreachable!()
    }
}

/// Execute the libvirt SSH command
pub fn run(global_opts: &crate::libvirt::LibvirtOptions, opts: LibvirtSshOpts) -> Result<()> {
    run_ssh_impl(global_opts, opts)
}

/// SSH implementation
pub fn run_ssh_impl(
    global_opts: &crate::libvirt::LibvirtOptions,
    opts: LibvirtSshOpts,
) -> Result<()> {
    debug!("Connecting to libvirt domain: {}", opts.domain_name);

    // Check if domain exists
    if !opts.check_domain_exists(global_opts)? {
        return Err(eyre!("Domain '{}' not found", opts.domain_name));
    }

    // Check if domain is running
    let state = opts.get_domain_state(global_opts)?;
    if state != "running" {
        return Err(eyre!(
            "Domain '{}' is not running (current state: {}). Start it first with: virsh start {}",
            opts.domain_name,
            state,
            opts.domain_name
        ));
    }

    // Extract SSH configuration from domain metadata
    let ssh_config = opts.extract_ssh_config(global_opts)?;

    // Connect via SSH with retries and console feedback
    opts.connect_ssh(global_opts, &ssh_config)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::xml_utils;

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
