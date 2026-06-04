//! Shared helpers for macOS/Windows VM management.
//!
//! Functions in this module are OS-independent (use `podman` and `ssh` CLI).
//! Modelled after `ssh_options.rs` — designed for future cross-platform sharing.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use color_eyre::{eyre::bail, eyre::eyre, eyre::Context, Result};
use tracing::info;

use crate::ssh_options::CommonSshOptions;

/// SSH connection timeout (shared by wait_for_ssh).
pub const SSH_TIMEOUT: Duration = Duration::from_secs(240);

/// Detect the currently active podman machine name.
pub fn detect_machine_name() -> Result<String> {
    let output = Command::new("podman")
        .args(["machine", "info", "--format", "{{.Host.CurrentMachine}}"])
        .output()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        bail!("no podman machine is running");
    }
    Ok(name)
}

/// Detect the podman machine VM type (e.g. "hyperv", "wsl", "libkrun", "applehv").
#[allow(dead_code)]
pub fn detect_podman_vmtype() -> Result<String> {
    let output = Command::new("podman")
        .args(["machine", "info", "--format", "{{.Host.VMType}}"])
        .output()?;
    let vmtype = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase();
    if vmtype.is_empty() {
        bail!("could not detect podman machine VM type");
    }
    Ok(vmtype)
}

/// Check if the podman machine is running as root (UID 0).
pub fn is_machine_rootful(machine: &str) -> bool {
    Command::new("podman")
        .args(["machine", "ssh", machine, "id", "-u"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

/// Parse memory specification string (e.g. "4G", "2048M") to megabytes.
pub fn parse_memory_to_mb(s: &str) -> Result<u32> {
    let s = s.trim();
    if let Some(n) = s.strip_suffix('G').or_else(|| s.strip_suffix('g')) {
        Ok((n.parse::<f64>()? * 1024.0) as u32)
    } else if let Some(n) = s.strip_suffix('M').or_else(|| s.strip_suffix('m')) {
        Ok(n.parse::<f64>()? as u32)
    } else {
        Ok(s.parse::<u32>()?)
    }
}

/// Return sensible default vCPU count based on available host parallelism.
pub fn default_vcpus() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(2)
}

/// Ensure image exists locally (pulling if needed) and return its short digest.
pub fn ensure_image_and_get_digest(image: &str) -> Result<String> {
    let status = Command::new("podman")
        .args(["image", "exists", image])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        info!("pulling image {}...", image);
        if !Command::new("podman")
            .args(["pull", image])
            .status()?
            .success()
        {
            bail!("failed to pull image: {}", image);
        }
    }
    let output = Command::new("podman")
        .args(["image", "inspect", "--format", "{{.Digest}}", image])
        .output()?;
    let digest = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if digest.is_empty() {
        bail!("failed to get image digest: {}", image);
    }
    Ok(digest
        .trim_start_matches("sha256:")
        .chars()
        .take(16)
        .collect())
}

/// Wait for SSH to become available on the given port.
pub fn wait_for_ssh(port: u16, key_path: &Path, user: &str) -> Result<()> {
    let ssh_opts = CommonSshOptions::default();
    let user_host = format!("{}@localhost", user);
    info!("waiting for SSH on port {}...", port);
    let start = std::time::Instant::now();
    let mut attempt = 0u32;
    loop {
        if start.elapsed() > SSH_TIMEOUT {
            bail!("SSH connection timeout ({}s)", SSH_TIMEOUT.as_secs());
        }
        let mut cmd = Command::new("ssh");
        cmd.args(["-p", &port.to_string(), "-i", &key_path.to_string_lossy()]);
        ssh_opts.apply_to_command(&mut cmd);
        cmd.args(["-o", "BatchMode=yes", &user_host, "true"]);
        if let Ok(s) = cmd.stdout(Stdio::null()).stderr(Stdio::null()).status() {
            if s.success() {
                info!("SSH connected after {}s", start.elapsed().as_secs());
                return Ok(());
            }
        }
        let backoff = if attempt < 2 {
            500
        } else if attempt < 4 {
            1000
        } else {
            2000
        };
        std::thread::sleep(Duration::from_millis(backoff));
        attempt += 1;
    }
}

/// Execute a command via SSH and return the exit status.
pub fn run_ssh_command(
    port: u16,
    key_path: &Path,
    user: &str,
    command: &str,
) -> Result<std::process::ExitStatus> {
    let ssh_opts = CommonSshOptions::default();
    let user_host = format!("{}@localhost", user);
    let mut cmd = Command::new("ssh");
    cmd.args(["-p", &port.to_string(), "-i", &key_path.to_string_lossy()]);
    ssh_opts.apply_to_command(&mut cmd);
    cmd.args(["-o", "BatchMode=yes", &user_host, command]);
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| eyre!("ssh failed: {}", e))
}

/// Start an interactive SSH session with TTY allocation.
pub fn run_ssh_interactive(
    port: u16,
    key_path: &Path,
    user: &str,
) -> Result<std::process::ExitStatus> {
    let ssh_opts = CommonSshOptions::default();
    let user_host = format!("{}@localhost", user);
    let mut cmd = Command::new("ssh");
    cmd.args(["-p", &port.to_string(), "-i", &key_path.to_string_lossy()]);
    ssh_opts.apply_to_command(&mut cmd);
    cmd.args(["-t", &user_host]);
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| eyre!("ssh failed: {}", e))
}

/// Remove a file, ignoring NotFound errors.
pub fn remove_file_if_exists(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::debug!("failed to remove {}: {}", path.display(), e);
        }
    }
}

/// Generate an SSH keypair and return the public key content.
pub fn generate_ssh_keypair(key_path: &Path) -> Result<String> {
    let pub_path = key_path.with_extension(
        key_path
            .extension()
            .map(|e| format!("{}.pub", e.to_string_lossy()))
            .unwrap_or_else(|| "pub".to_string()),
    );
    remove_file_if_exists(key_path);
    remove_file_if_exists(&pub_path);
    let status = Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-N",
            "",
            "-q",
            "-f",
            &key_path.to_string_lossy(),
        ])
        .status()?;
    if !status.success() {
        bail!("ssh-keygen failed");
    }
    let pubkey = std::fs::read_to_string(&pub_path)?.trim().to_string();
    Ok(pubkey)
}

/// Sanitize a container image name into a valid VM name.
pub fn sanitize_vm_name(image: &str) -> String {
    image
        .split('/')
        .last()
        .unwrap_or(image)
        .replace(':', "-")
        .replace('.', "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Parse a size string (e.g. "10G", "20GB", "5120M", "1TB") to bytes.
pub fn parse_size(size_str: &str) -> Result<u64> {
    let s = size_str.trim();
    if s.is_empty() {
        bail!("empty size string");
    }
    if let Ok(n) = s.parse::<u64>() {
        return Ok(n);
    }
    let upper = s.to_uppercase();
    let (num_str, multiplier) = if let Some(n) = upper.strip_suffix("TB") {
        (n, 1024_u64.pow(4))
    } else if let Some(n) = upper.strip_suffix("GB") {
        (n, 1024_u64 * 1024 * 1024)
    } else if let Some(n) = upper.strip_suffix("MB") {
        (n, 1024_u64 * 1024)
    } else if let Some(n) = upper.strip_suffix("KB") {
        (n, 1024_u64)
    } else if let Some(n) = upper.strip_suffix('T') {
        (n, 1024_u64.pow(4))
    } else if let Some(n) = upper.strip_suffix('G') {
        (n, 1024_u64 * 1024 * 1024)
    } else if let Some(n) = upper.strip_suffix('M') {
        (n, 1024_u64 * 1024)
    } else if let Some(n) = upper.strip_suffix('K') {
        (n, 1024_u64)
    } else if let Some(n) = upper.strip_suffix('B') {
        (n, 1)
    } else {
        bail!("invalid size format: '{}' (use e.g. 20G, 5120M, 1TB)", s);
    };
    let num: u64 = num_str
        .trim()
        .parse()
        .with_context(|| format!("invalid number in size: '{}'", num_str))?;
    Ok(num * multiplier)
}

/// Container image name for the nbdkit EROFS plugin.
pub const NBDKIT_IMAGE: &str = "localhost/bcvk-nbdkit:latest";

/// Generate a shell script that checks for and builds the nbdkit container image.
///
/// The caller provides the plugin `.so` binary via `plugin_so` (typically from
/// `include_bytes!` in a platform-specific module). The script:
/// 1. Checks if the image already exists (early exit if so)
/// 2. Writes the `.so` to a temp path via base64
/// 3. Builds a container image with nbdkit + the plugin baked in
/// 4. Cleans up the temp file
pub fn nbdkit_setup_script(plugin_so: &[u8]) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(plugin_so);
    format!(
        "set -e; \
         if podman image exists {image}; then exit 0; fi; \
         mkdir -p /var/tmp/bcvk; \
         printf '%s' '{b64}' | base64 -d > /var/tmp/bcvk/plugin.so; \
         printf 'FROM quay.io/fedora/fedora:latest\\nRUN dnf install -y nbdkit nbdkit-basic-plugins && dnf clean all\\nCOPY plugin.so /plugin.so\\n' | \
         podman build -t {image} -f - /var/tmp/bcvk; \
         rm -f /var/tmp/bcvk/plugin.so",
        image = NBDKIT_IMAGE,
        b64 = b64,
    )
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("10G").unwrap(), 10 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("20GB").unwrap(), 20 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("5120M").unwrap(), 5120 * 1024 * 1024);
        assert_eq!(parse_size("512MB").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_size("1024K").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("1TB").unwrap(), 1024_u64.pow(4));
        assert_eq!(parse_size("1073741824").unwrap(), 1073741824);
        assert_eq!(parse_size("100B").unwrap(), 100);
        assert!(parse_size("abc").is_err());
        assert!(parse_size("10X").is_err());
        assert!(parse_size("").is_err());
    }

    #[test]
    fn test_parse_memory_to_mb() {
        assert_eq!(parse_memory_to_mb("4G").unwrap(), 4096);
        assert_eq!(parse_memory_to_mb("2048M").unwrap(), 2048);
        assert_eq!(parse_memory_to_mb("512").unwrap(), 512);
        assert_eq!(parse_memory_to_mb("1g").unwrap(), 1024);
        assert_eq!(parse_memory_to_mb("256m").unwrap(), 256);
    }

    #[test]
    fn test_parse_memory_to_mb_errors() {
        assert!(parse_memory_to_mb("abc").is_err());
    }

    #[test]
    fn test_default_vcpus() {
        let vcpus = default_vcpus();
        assert!(vcpus >= 1);
    }

    #[test]
    fn test_sanitize_vm_name() {
        assert_eq!(
            sanitize_vm_name("quay.io/fedora/fedora-bootc:latest"),
            "fedora-bootc-latest"
        );
        assert_eq!(sanitize_vm_name("centos:stream10"), "centos-stream10");
        assert_eq!(sanitize_vm_name("simple"), "simple");
    }
}
