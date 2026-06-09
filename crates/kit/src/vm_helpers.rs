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
            100
        } else if attempt < 4 {
            200
        } else {
            500
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

// --- NBD server helpers (shared by macOS/Windows) ---

/// Shell-escape a string for safe embedding in shell commands.
pub fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Compute a fast hash of binary data for deployment change detection.
pub fn binary_hash(data: &[u8]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    data.hash(&mut h);
    let hash1 = h.finish();
    data.len().hash(&mut h);
    let hash2 = h.finish();
    format!("{:016x}{:016x}", hash1, hash2)
}

/// Run a command inside the podman machine via SSH (best-effort, no output).
pub fn machine_ssh(machine: &str, cmd: &str) -> Result<()> {
    let status = Command::new("podman")
        .args(["machine", "ssh", machine, "--", cmd])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("podman machine ssh")?;
    if !status.success() {
        bail!("machine ssh command failed");
    }
    Ok(())
}

/// Run a command inside the podman machine via SSH and capture output.
pub fn machine_ssh_output(machine: &str, cmd: &str) -> Result<std::process::Output> {
    Command::new("podman")
        .args(["machine", "ssh", machine, "--", cmd])
        .output()
        .context("podman machine ssh")
}

/// Get the merged overlay path from podman image mount.
pub fn get_merged_path(machine: &str, rootful: bool, image: &str) -> Result<String> {
    let output = if rootful {
        Command::new("podman")
            .args([
                "machine", "ssh", machine, "--", "podman", "image", "mount", image,
            ])
            .output()
            .context("podman image mount")?
    } else {
        Command::new("podman")
            .args([
                "machine", "ssh", machine, "--", "podman", "unshare", "podman", "image", "mount",
                image,
            ])
            .output()
            .context("podman image mount")?
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("podman image mount failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Deploy the NBD server binary to the podman machine (idempotent, hash-checked).
pub fn deploy_nbd_server(machine: &str, binary: &[u8]) -> Result<()> {
    use base64::Engine;
    let hash = binary_hash(binary);
    let b64 = base64::engine::general_purpose::STANDARD.encode(binary);
    let script = format!(
        "set -e; \
         mkdir -p /var/tmp/bcvk; \
         H=/var/tmp/bcvk/bcvk-nbd.sha256; \
         if [ -f \"$H\" ] && [ \"$(cat \"$H\")\" = '{hash}' ]; then exit 0; fi; \
         printf '%s' '{b64}' | base64 -d > /var/tmp/bcvk/bcvk-nbd; \
         chmod +x /var/tmp/bcvk/bcvk-nbd; \
         chcon -t bin_t /var/tmp/bcvk/bcvk-nbd 2>/dev/null || true; \
         printf '{hash}' > \"$H\"",
        hash = hash,
        b64 = b64,
    );
    info!("deploying nbd server to podman machine...");
    let mut child = Command::new("podman")
        .args(["machine", "ssh", machine, "--", "bash", "-s"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("nbd server deploy to podman machine")?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(script.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!(
            "nbd server deploy failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Start the NBD server as a systemd-run unit inside the podman machine.
///
/// `listen_arg` controls the transport: `"--port {nbd_port}"` for TCP (macOS)
/// or `"--vsock --port {vsock_port}"` for vsock (Windows).
pub fn start_nbd_unit(
    machine: &str,
    unit_name: &str,
    merged_path: &str,
    cmdline: &str,
    ssh_pubkey: &str,
    listen_arg: &str,
) -> Result<()> {
    if let Err(e) = machine_ssh(
        machine,
        &format!(
            "systemctl stop {u} 2>/dev/null; systemctl reset-failed {u} 2>/dev/null",
            u = unit_name
        ),
    ) {
        tracing::debug!("pre-cleanup of unit {} failed: {}", unit_name, e);
    }

    let cmdline_esc = shell_escape(cmdline);
    let mut ssh_args = String::new();
    if !ssh_pubkey.is_empty() {
        ssh_args = format!(" --ssh-pubkey {}", shell_escape(ssh_pubkey));
    }

    let start_cmd = format!(
        "systemd-run --unit={unit} --service-type=simple --quiet \
         --property=LimitNOFILE=524288 \
         /var/tmp/bcvk/bcvk-nbd {listen} --dir {merged} \
         --cmdline {cmdline}{ssh}",
        unit = unit_name,
        listen = listen_arg,
        merged = merged_path,
        cmdline = cmdline_esc,
        ssh = ssh_args,
    );
    let output = machine_ssh_output(machine, &start_cmd)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("failed to start nbd server: {}", stderr.trim());
    }
    Ok(())
}

/// Stop an NBD server systemd-run unit (best-effort).
pub fn stop_nbd_unit(machine: &str, unit_name: &str) {
    if let Err(e) = machine_ssh(
        machine,
        &format!(
            "systemctl stop {u} 2>/dev/null; systemctl reset-failed {u} 2>/dev/null",
            u = unit_name
        ),
    ) {
        tracing::debug!("stop_nbd_unit failed for {}: {}", unit_name, e);
    }
}

/// Check if a systemd-run unit has died.
pub fn is_nbd_unit_dead(machine: &str, unit_name: &str) -> bool {
    if let Ok(out) = machine_ssh_output(machine, &format!("systemctl is-active {}", unit_name)) {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let state = stdout.trim();
        state == "inactive" || state == "failed"
    } else {
        false
    }
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
