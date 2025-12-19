//! SSH integration for bcvk VMs

use camino::{Utf8Path, Utf8PathBuf};
use color_eyre::{eyre::eyre, Result};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use tracing::debug;

use crate::CONTAINER_STATEDIR;

/// Combine multiple command arguments into a properly escaped shell command string
///
/// This is necessary because SSH protocol sends commands as strings, not argument arrays.
/// When bcvk receives multiple arguments like ["/bin/sh", "-c", "echo hello; sleep 5"],
/// they must be combined into a single string that will be correctly interpreted by the
/// remote shell.
///
/// Uses the `shlex` crate for robust POSIX shell escaping.
pub fn shell_escape_command(args: &[String]) -> Result<String, shlex::QuoteError> {
    shlex::try_join(args.iter().map(|s| s.as_str()))
}

/// Represents an SSH keypair with file paths and public key content
#[derive(Debug, Clone)]
pub struct SshKeyPair {
    /// Path to the private key file
    #[allow(dead_code)]
    pub private_key_path: Utf8PathBuf,
    /// Path to the public key file (typically private_key_path + ".pub")
    pub public_key_path: Utf8PathBuf,
}

/// Generate a new RSA SSH keypair in the specified directory
///
/// Creates a new 4096-bit RSA SSH keypair using the system's `ssh-keygen` command.
/// The private key is created with secure permissions (0600) and no passphrase to
/// enable automated use cases.
pub fn generate_ssh_keypair(output_dir: &Utf8Path, key_name: &str) -> Result<SshKeyPair> {
    // Create output directory if it doesn't exist
    fs::create_dir_all(output_dir.as_std_path())?;

    let private_key_path = output_dir.join(key_name);
    let public_key_path = output_dir.join(format!("{}.pub", key_name));

    debug!("Generating SSH keypair at {:?}", private_key_path);

    // Generate RSA key with ssh-keygen
    let output = Command::new("ssh-keygen")
        .args([
            "-t",
            "rsa",
            "-b",
            "4096", // Use 4096-bit RSA for security
            "-f",
            private_key_path.as_str(),
            "-N",
            "", // No passphrase
            "-C",
            &format!("bcvk-{}", key_name), // Comment
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!("ssh-keygen failed: {}", stderr));
    }

    // Set secure permissions on private key
    let metadata = fs::metadata(private_key_path.as_std_path())?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o600); // Read/write for owner only
    fs::set_permissions(private_key_path.as_std_path(), permissions)?;

    debug!("Generated SSH keypair successfully");

    Ok(SshKeyPair {
        private_key_path,
        public_key_path,
    })
}

pub fn generate_default_keypair() -> Result<SshKeyPair> {
    generate_ssh_keypair(Utf8Path::new(CONTAINER_STATEDIR), "ssh")
}

/// Connect to VM via container-based SSH access
///
/// Establishes an SSH connection to a VM by executing SSH commands inside the
/// container that hosts the VM. This is the primary connection method for bcvk
/// VMs and provides isolated, secure access without requiring direct host network
/// configuration.
///
/// # Arguments
///
/// * `container_name` - Name of the podman container hosting the VM
/// * `args` - Additional arguments to pass to the SSH command
/// * `options` - SSH connection configuration options
///
/// # Example
///
/// ```rust,no_run
/// use bootc_kit::ssh::{connect, SshConnectionOptions};
///
/// // Interactive SSH session with default options
/// connect("bootc-vm-abc123", vec![], &SshConnectionOptions::default())?;
///
/// // Run a specific command
/// let args = vec!["systemctl".to_string(), "status".to_string()];
/// connect("bootc-vm-abc123", args, &SshConnectionOptions::default())?;
/// ```
pub fn connect(
    container_name: &str,
    args: Vec<String>,
    options: &SshConnectionOptions,
) -> Result<std::process::ExitStatus> {
    debug!("Connecting to VM via container: {}", container_name);

    // Verify container exists and is running
    verify_container_running(container_name)?;

    // Build podman exec command
    let mut cmd = Command::new("podman");
    if options.allocate_tty {
        cmd.args(["exec", "-it", container_name, "ssh"]);
    } else {
        cmd.args(["exec", container_name, "ssh"]);
    }

    // SSH key path (hardcoded for container environment)
    let keypath = Utf8Path::new("/run/tmproot")
        .join(CONTAINER_STATEDIR.trim_start_matches('/'))
        .join("ssh");
    cmd.args(["-i", keypath.as_str()]);

    // Apply common SSH options
    options.common.apply_to_command(&mut cmd);

    // No prompts from SSH
    cmd.args(["-o", "BatchMode=yes"]);

    // Even if we're providing a remote command, always allocate a tty
    // so progress bars work because we're running synchronously.
    if options.allocate_tty {
        cmd.arg("-t");
    }

    // Connect to VM via QEMU port forwarding on localhost
    cmd.arg("root@127.0.0.1");
    cmd.args(["-p", "2222"]);

    // Add any additional arguments
    let ssh_args = build_ssh_command(&args)?;
    if !ssh_args.is_empty() {
        debug!("Adding SSH arguments: {:?}", ssh_args);
        cmd.args(&ssh_args);
    }

    debug!("Executing: podman {:?}", cmd.get_args().collect::<Vec<_>>());
    debug!(
        "Full command line: podman {}",
        cmd.get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(" ")
    );

    // Suppress output if requested (useful for connectivity testing)
    if options.suppress_output {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    } else {
        // Explicitly inherit stdout/stderr to prevent them from being closed
        cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    }

    // Execute the command and return status
    cmd.status()
        .map_err(|e| eyre!("Failed to execute SSH command: {}", e))
}

/// Convenience function for connecting with error handling (non-zero exit = error)
pub fn connect_via_container(container_name: &str, args: Vec<String>) -> Result<()> {
    let status = connect(container_name, args, &SshConnectionOptions::default())?;
    if !status.success() {
        return Err(eyre!(
            "SSH connection failed with exit code: {:?}",
            status.code()
        ));
    }
    Ok(())
}

/// SSH connection configuration options
#[derive(Debug, Clone)]
pub struct SshConnectionOptions {
    /// Common SSH options shared across implementations
    pub common: CommonSshOptions,
    /// Enable/disable TTY allocation (default: true)
    pub allocate_tty: bool,
    /// Suppress output to stdout/stderr (default: false)
    pub suppress_output: bool,
}

/// Common SSH options that can be shared between different SSH implementations
#[derive(Debug, Clone)]
pub struct CommonSshOptions {
    /// Use strict host key checking
    pub strict_host_keys: bool,
    /// SSH connection timeout in seconds
    pub connect_timeout: u32,
    /// Server alive interval in seconds
    pub server_alive_interval: u32,
    /// SSH log level
    pub log_level: String,
    /// Additional SSH options as key-value pairs
    pub extra_options: Vec<(String, String)>,
}

impl Default for CommonSshOptions {
    fn default() -> Self {
        Self {
            strict_host_keys: false,
            connect_timeout: 1,
            server_alive_interval: 60,
            log_level: "ERROR".to_string(),
            extra_options: vec![],
        }
    }
}

impl CommonSshOptions {
    /// Apply these options to an SSH command
    pub fn apply_to_command(&self, cmd: &mut std::process::Command) {
        // Basic security options
        cmd.args(["-o", "IdentitiesOnly=yes"]);
        cmd.args(["-o", "PasswordAuthentication=no"]);
        cmd.args(["-o", "KbdInteractiveAuthentication=no"]);
        cmd.args(["-o", "GSSAPIAuthentication=no"]);

        // Connection options
        cmd.args(["-o", &format!("ConnectTimeout={}", self.connect_timeout)]);
        cmd.args([
            "-o",
            &format!("ServerAliveInterval={}", self.server_alive_interval),
        ]);
        cmd.args(["-o", &format!("LogLevel={}", self.log_level)]);

        // Host key checking
        if !self.strict_host_keys {
            cmd.args(["-o", "StrictHostKeyChecking=no"]);
            cmd.args(["-o", "UserKnownHostsFile=/dev/null"]);
        }

        // Add extra SSH options
        for (key, value) in &self.extra_options {
            cmd.args(["-o", &format!("{}={}", key, value)]);
        }
    }
}

impl Default for SshConnectionOptions {
    fn default() -> Self {
        Self {
            common: CommonSshOptions::default(),
            allocate_tty: true,
            suppress_output: false,
        }
    }
}

impl SshConnectionOptions {
    /// Create options suitable for quick connectivity tests (short timeout, no TTY)
    pub fn for_connectivity_test() -> Self {
        Self {
            common: CommonSshOptions {
                strict_host_keys: false,
                connect_timeout: 2,
                server_alive_interval: 60,
                log_level: "ERROR".to_string(),
                extra_options: vec![],
            },
            allocate_tty: false,
            suppress_output: true,
        }
    }
}

/// Verify that a container exists and is running
fn verify_container_running(container_name: &str) -> Result<()> {
    let status = Command::new("podman")
        .args(["inspect", container_name, "--format", "{{.State.Status}}"])
        .output()
        .map_err(|e| eyre!("Failed to check container status: {}", e))?;

    if !status.status.success() {
        return Err(eyre!("Container '{}' not found", container_name));
    }

    let container_status = String::from_utf8_lossy(&status.stdout).trim().to_string();
    if container_status != "running" {
        return Err(eyre!(
            "Container '{}' is not running (status: {})",
            container_name,
            container_status
        ));
    }

    Ok(())
}

/// Build SSH command with proper argument handling
fn build_ssh_command(args: &[String]) -> Result<Vec<String>> {
    if args.is_empty() {
        return Ok(vec![]);
    }

    let mut ssh_args = vec!["--".to_string()];

    // If we have multiple arguments, we need to properly combine them into a single
    // command string that will survive shell parsing on the remote side.
    // This is because SSH protocol sends commands as strings, not argument arrays.
    if args.len() > 1 {
        // Combine arguments with proper shell escaping
        let combined_command = shell_escape_command(args)
            .map_err(|e| eyre!("Failed to escape shell command: {}", e))?;
        debug!("Combined escaped command: {}", combined_command);
        ssh_args.push(combined_command);
    } else {
        // Single argument can be passed directly
        ssh_args.extend(args.iter().cloned());
    }

    Ok(ssh_args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_generate_ssh_keypair() {
        let temp_dir = TempDir::new().unwrap();
        let key_pair =
            generate_ssh_keypair(Utf8Path::from_path(temp_dir.path()).unwrap(), "test_key")
                .unwrap();

        // Check that files exist
        assert!(key_pair.private_key_path.exists());
        assert!(key_pair.public_key_path.exists());

        let content = std::fs::read_to_string(key_pair.public_key_path.as_std_path()).unwrap();
        // Check that public key starts with expected format
        assert!(content.starts_with("ssh-rsa"));

        // Check private key permissions
        let metadata = std::fs::metadata(key_pair.private_key_path.as_std_path()).unwrap();
        let permissions = metadata.permissions();
        assert_eq!(permissions.mode() & 0o777, 0o600);
    }

    #[test]
    fn test_ssh_connection_options() {
        // Test default options
        let default_opts = SshConnectionOptions::default();
        assert_eq!(default_opts.common.connect_timeout, 1);
        assert!(default_opts.allocate_tty);
        assert_eq!(default_opts.common.log_level, "ERROR");
        assert!(default_opts.common.extra_options.is_empty());
        assert!(!default_opts.suppress_output);

        // Test connectivity test options
        let test_opts = SshConnectionOptions::for_connectivity_test();
        assert_eq!(test_opts.common.connect_timeout, 2);
        assert!(!test_opts.allocate_tty);
        assert_eq!(test_opts.common.log_level, "ERROR");
        assert!(test_opts.common.extra_options.is_empty());
        assert!(test_opts.suppress_output);

        // Test custom options
        let mut custom_opts = SshConnectionOptions::default();
        custom_opts.common.connect_timeout = 10;
        custom_opts.allocate_tty = false;
        custom_opts.common.log_level = "DEBUG".to_string();
        custom_opts
            .common
            .extra_options
            .push(("ServerAliveInterval".to_string(), "30".to_string()));

        assert_eq!(custom_opts.common.connect_timeout, 10);
        assert!(!custom_opts.allocate_tty);
        assert_eq!(custom_opts.common.log_level, "DEBUG");
        assert_eq!(custom_opts.common.extra_options.len(), 1);
        assert_eq!(
            custom_opts.common.extra_options[0],
            ("ServerAliveInterval".to_string(), "30".to_string())
        );
    }

    #[test]
    fn test_shell_escape_command() {
        // Single argument
        assert_eq!(shell_escape_command(&["echo".to_string()]).unwrap(), "echo");

        // Multiple simple arguments
        assert_eq!(
            shell_escape_command(&["/bin/sh".to_string(), "-c".to_string()]).unwrap(),
            "/bin/sh -c"
        );

        // Arguments with special characters - shlex uses single quotes for POSIX compliance
        let result = shell_escape_command(&[
            "/bin/sh".to_string(),
            "-c".to_string(),
            "echo hello; sleep 5; echo world".to_string(),
        ])
        .unwrap();
        assert_eq!(result, "/bin/sh -c 'echo hello; sleep 5; echo world'");

        // Test that shlex properly handles quotes and spaces
        let result2 = shell_escape_command(&[
            "echo".to_string(),
            "hello world".to_string(),
            "it's working".to_string(),
        ])
        .unwrap();
        assert_eq!(result2, "echo 'hello world' \"it's working\"");

        // Test edge case with single quotes - shlex uses double quotes
        let result3 =
            shell_escape_command(&["echo".to_string(), "don't do this".to_string()]).unwrap();
        assert_eq!(result3, "echo \"don't do this\"");

        // Test system command like in the integration test - shell operators get quoted
        let result4 = shell_escape_command(&[
            "systemctl".to_string(),
            "is-system-running".to_string(),
            "||".to_string(),
            "true".to_string(),
        ])
        .unwrap();
        assert_eq!(result4, "systemctl is-system-running '||' true");
    }
}
