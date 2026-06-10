//! Cross-platform SSH option types shared between different backends.
//!
//! Extracted from ssh.rs to allow macOS and Windows backends to share
//! SSH option types without pulling in Linux-specific dependencies.

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
