//! Cross-platform SSH option types shared between Linux and macOS backends.
//!
//! Extracted from ssh.rs to avoid pulling in Linux-only dependencies on macOS.

/// Common SSH options that can be shared between different SSH implementations
#[derive(Debug, Clone)]
#[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn apply_to_command(&self, cmd: &mut std::process::Command) {
        cmd.args(["-o", "IdentitiesOnly=yes"]);
        cmd.args(["-o", "PasswordAuthentication=no"]);
        cmd.args(["-o", "KbdInteractiveAuthentication=no"]);
        cmd.args(["-o", "GSSAPIAuthentication=no"]);

        cmd.args(["-o", &format!("ConnectTimeout={}", self.connect_timeout)]);
        cmd.args([
            "-o",
            &format!("ServerAliveInterval={}", self.server_alive_interval),
        ]);
        cmd.args(["-o", &format!("LogLevel={}", self.log_level)]);

        if !self.strict_host_keys {
            cmd.args(["-o", "StrictHostKeyChecking=no"]);
            cmd.args(["-o", "UserKnownHostsFile=/dev/null"]);
        }

        for (key, value) in &self.extra_options {
            cmd.args(["-o", &format!("{}={}", key, value)]);
        }
    }
}

/// SSH connection configuration options
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SshConnectionOptions {
    /// Common SSH options shared across implementations
    pub common: CommonSshOptions,
    /// Enable/disable TTY allocation (default: true)
    pub allocate_tty: bool,
    /// Suppress output to stdout/stderr (default: false)
    pub suppress_output: bool,
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
    #[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_common_ssh_options_default() {
        let opts = CommonSshOptions::default();
        assert!(!opts.strict_host_keys);
        assert_eq!(opts.connect_timeout, 1);
        assert_eq!(opts.server_alive_interval, 60);
        assert_eq!(opts.log_level, "ERROR");
        assert!(opts.extra_options.is_empty());
    }

    #[test]
    fn test_connectivity_test_options() {
        let opts = SshConnectionOptions::for_connectivity_test();
        assert_eq!(opts.common.connect_timeout, 2);
        assert!(!opts.allocate_tty);
        assert!(opts.suppress_output);
    }

    #[test]
    fn test_apply_to_command() {
        let opts = CommonSshOptions::default();
        let mut cmd = std::process::Command::new("ssh");
        opts.apply_to_command(&mut cmd);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"IdentitiesOnly=yes".to_string()));
        assert!(args.contains(&"PasswordAuthentication=no".to_string()));
        assert!(args.contains(&"StrictHostKeyChecking=no".to_string()));
        assert!(args.contains(&"ConnectTimeout=1".to_string()));
    }
}
