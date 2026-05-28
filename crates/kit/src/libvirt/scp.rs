//! SCP file transfer to/from libvirt domains with embedded SSH credentials
//!
//! This module provides functionality to copy files to/from libvirt domains
//! that were created with SSH key injection, automatically retrieving SSH
//! credentials from domain XML metadata.

use clap::Parser;
use color_eyre::{eyre::eyre, Result};
use std::process::Command;
use std::time::Instant;
use tracing::debug;

// Reuse SSH constants and helpers from the ssh module
use super::ssh::{wait_for_ssh_ready, LibvirtSshOpts};

/// Configuration options for SCP file transfer to/from a libvirt domain
#[derive(Debug, Parser)]
pub struct LibvirtScpOpts {
    /// Name of the libvirt domain to connect to
    pub domain_name: String,

    /// Source path (use domain: prefix for remote paths, e.g. `/local/file` or `domain:/remote/file`)
    pub source: String,

    /// Destination path (use domain: prefix for remote paths, e.g. `/local/file` or `domain:/remote/file`)
    pub destination: String,

    /// SSH username to use for connection (defaults to 'root')
    #[clap(long, default_value = "root")]
    pub user: String,

    /// Copy directories recursively
    #[clap(short, long)]
    pub recursive: bool,

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
}

/// Resolve a user-facing path, replacing `domain:` with `user@host:`.
///
/// Users write:
///   bcvk libvirt scp myvm domain:/etc/hostname ./hostname
///
/// This function turns `domain:/etc/hostname` into `root@127.0.0.1:/etc/hostname`
/// (or whichever user was requested).
fn resolve_scp_path(raw: &str, user: &str) -> String {
    if let Some(remote_path) = raw.strip_prefix("domain:") {
        format!("{}@127.0.0.1:{}", user, remote_path)
    } else {
        raw.to_string()
    }
}

impl LibvirtScpOpts {
    /// Parse extra options into key-value pairs
    fn parse_extra_options(&self) -> Result<Vec<(String, String)>> {
        let mut parsed = Vec::new();
        for option in &self.extra_options {
            if let Some((key, value)) = option.split_once('=') {
                parsed.push((key.to_string(), value.to_string()));
            } else {
                return Err(eyre!(
                    "Invalid extra option format '{}'. Expected 'key=value'",
                    option
                ));
            }
        }
        Ok(parsed)
    }

    /// Build an SCP command with the correct key, port, and options.
    fn build_scp_command(
        &self,
        temp_key: &tempfile::NamedTempFile,
        ssh_port: u16,
        parsed_extra_options: &[(String, String)],
    ) -> Command {
        let mut cmd = Command::new("scp");

        // Identity / port
        cmd.arg("-i").arg(temp_key.path());
        cmd.arg("-P").arg(ssh_port.to_string());

        // Recursive flag
        if self.recursive {
            cmd.arg("-r");
        }

        // Reuse the common SSH option plumbing via `-o`
        let common_opts = crate::ssh::CommonSshOptions {
            strict_host_keys: self.strict_host_keys,
            connect_timeout: self.timeout,
            server_alive_interval: super::ssh::SSH_SERVER_ALIVE_INTERVAL,
            log_level: self.log_level.clone(),
            extra_options: parsed_extra_options.to_vec(),
        };
        common_opts.apply_to_command(&mut cmd);

        cmd
    }
}

/// Execute the libvirt SCP command
pub fn run(global_opts: &crate::libvirt::LibvirtOptions, opts: LibvirtScpOpts) -> Result<()> {
    debug!("SCP file transfer for libvirt domain: {}", opts.domain_name);

    // Validate that exactly one side references the domain
    let source_is_remote = opts.source.starts_with("domain:");
    let dest_is_remote = opts.destination.starts_with("domain:");

    if source_is_remote == dest_is_remote {
        return Err(eyre!(
            "Exactly one of source or destination must use the domain: prefix to reference the remote VM.\n\
             Examples:\n  \
               bcvk libvirt scp myvm domain:/etc/hostname ./hostname\n  \
               bcvk libvirt scp myvm ./file.txt domain:/tmp/file.txt"
        ));
    }

    // Reuse the SSH infrastructure for domain checks and credential extraction
    // by constructing a temporary LibvirtSshOpts (with no command).
    let ssh_helper = LibvirtSshOpts {
        domain_name: opts.domain_name.clone(),
        user: opts.user.clone(),
        command: vec![],
        strict_host_keys: opts.strict_host_keys,
        timeout: opts.timeout,
        log_level: opts.log_level.clone(),
        extra_options: opts.extra_options.clone(),
        suppress_output: true,
    };

    // Check domain exists
    if !ssh_helper.check_domain_exists(global_opts)? {
        return Err(eyre!("Domain '{}' not found", opts.domain_name));
    }

    // Check domain is running
    let state = ssh_helper.get_domain_state(global_opts)?;
    if state != "running" {
        return Err(eyre!(
            "Domain '{}' is not running (current state: {}). Start it first with: virsh start {}",
            opts.domain_name,
            state,
            opts.domain_name
        ));
    }

    // Extract SSH config (key, port) from domain metadata
    let ssh_config = ssh_helper.extract_ssh_config(global_opts)?;

    // Create temp key file
    let temp_key = ssh_helper.create_temp_ssh_key(&ssh_config)?;

    let parsed_extra_options = opts.parse_extra_options()?;

    // Wait for SSH to be ready (shared helper with ssh subcommand)
    let common_opts = crate::ssh::CommonSshOptions {
        strict_host_keys: opts.strict_host_keys,
        connect_timeout: opts.timeout,
        server_alive_interval: super::ssh::SSH_SERVER_ALIVE_INTERVAL,
        log_level: opts.log_level.clone(),
        extra_options: parsed_extra_options.clone(),
    };
    wait_for_ssh_ready(
        &ssh_config,
        temp_key.path(),
        &opts.user,
        &common_opts,
        &opts.domain_name,
    )?;
    let start_time = Instant::now();

    // Build and exec the SCP command
    let resolved_source = resolve_scp_path(&opts.source, &opts.user);
    let resolved_dest = resolve_scp_path(&opts.destination, &opts.user);

    debug!(
        "Running SCP: {} -> {} (port {})",
        resolved_source, resolved_dest, ssh_config.ssh_port
    );

    let mut scp_cmd = opts.build_scp_command(&temp_key, ssh_config.ssh_port, &parsed_extra_options);
    scp_cmd.arg(&resolved_source).arg(&resolved_dest);

    let status = scp_cmd
        .status()
        .map_err(|e| eyre!("Failed to execute scp command: {}", e))?;

    if !status.success() {
        return Err(eyre!("SCP failed with exit code: {:?}", status.code()));
    }

    debug!(
        "SCP completed successfully in {:.1}s",
        start_time.elapsed().as_secs_f64()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_scp_path_remote() {
        assert_eq!(
            resolve_scp_path("domain:/etc/hostname", "root"),
            "root@127.0.0.1:/etc/hostname"
        );
        assert_eq!(
            resolve_scp_path("domain:/tmp/file.txt", "alice"),
            "alice@127.0.0.1:/tmp/file.txt"
        );
    }

    #[test]
    fn test_resolve_scp_path_local() {
        assert_eq!(resolve_scp_path("/local/path", "root"), "/local/path");
        assert_eq!(resolve_scp_path("./relative", "root"), "./relative");
    }
}
