//! VirtioFS daemon management.
//!
//! Provides functionality for spawning and configuring virtiofsd daemons
//! that enable sharing host directories with QEMU guests via VirtIO-FS.

use camino::Utf8PathBuf;
use color_eyre::eyre::{eyre, Context};
use color_eyre::Result;
use tracing::debug;

/// VirtiofsD daemon configuration.
#[derive(Debug, Clone)]
pub struct VirtiofsConfig {
    /// Unix socket for QEMU communication.
    pub socket_path: Utf8PathBuf,
    /// Host directory to share.
    pub shared_dir: Utf8PathBuf,
    /// Enable debug output.
    pub debug: bool,
    /// Mount as read-only.
    pub readonly: bool,
    /// Optional log file path for virtiofsd output.
    pub log_file: Option<Utf8PathBuf>,
}

impl Default for VirtiofsConfig {
    fn default() -> Self {
        Self {
            socket_path: "/run/inner-shared/virtiofs.sock".into(),
            shared_dir: "/run/source-image".into(),
            debug: false,
            // We don't need to write to this, there's a transient overlay
            readonly: true,
            log_file: None,
        }
    }
}

/// Check if virtiofsd supports the --readonly flag.
async fn virtiofsd_supports_readonly(virtiofsd_binary: &str) -> bool {
    let output = tokio::process::Command::new(virtiofsd_binary)
        .arg("--help")
        .output()
        .await;

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            stdout.contains("--readonly") || stderr.contains("--readonly")
        }
        Err(_) => false,
    }
}

/// Spawn virtiofsd daemon process as tokio::process::Child.
///
/// Searches for binary in /usr/libexec, /usr/bin, /usr/local/bin.
/// Creates socket directory if needed, redirects output unless debug=true.
pub async fn spawn_virtiofsd_async(config: &VirtiofsConfig) -> Result<tokio::process::Child> {
    // Validate configuration
    validate_virtiofsd_config(config)?;

    // Try common virtiofsd binary locations
    let virtiofsd_paths = [
        "/usr/libexec/virtiofsd",
        "/usr/bin/virtiofsd",
        "/usr/local/bin/virtiofsd",
        "/usr/lib/virtiofsd",
    ];

    let virtiofsd_binary = virtiofsd_paths
        .iter()
        .find(|path| std::path::Path::new(path).exists())
        .ok_or_else(|| {
            eyre!(
                "virtiofsd binary not found. Searched paths: {}. Please install virtiofsd.",
                virtiofsd_paths.join(", ")
            )
        })?;

    // Check if virtiofsd supports --readonly flag
    let supports_readonly = virtiofsd_supports_readonly(virtiofsd_binary).await;
    debug!(
        "virtiofsd at {} supports --readonly: {}",
        virtiofsd_binary, supports_readonly
    );

    let mut cmd = tokio::process::Command::new(virtiofsd_binary);
    // SAFETY: This API is safe to call in a forked child.
    #[allow(unsafe_code)]
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::TERM))
                .map_err(Into::into)
        });
    }
    cmd.args([
        "--socket-path",
        config.socket_path.as_str(),
        "--shared-dir",
        config.shared_dir.as_str(),
        // Ensure we don't hit fd exhaustion
        "--cache=never",
        // Allowing mmap is needed in the general case for loading shared libraries
        // etc. This flag negotiates FUSE_DIRECT_IO_ALLOW_MMAP with the kernel (requires kernel 6.2+).
        // Per the documentation this is safe because the underlying filesystem tree is immutable.
        "--allow-mmap",
        // We always run in a container
        "--sandbox=none",
    ]);

    // Only add --readonly if requested and supported
    if config.readonly && supports_readonly {
        cmd.arg("--readonly");
    }

    // https://gitlab.com/virtio-fs/virtiofsd/-/issues/17 - this is the new default,
    // but we want to be compatible with older virtiofsd too.
    cmd.arg("--inode-file-handles=fallback");

    // Configure output redirection
    if let Some(log_file) = &config.log_file {
        // Create/open log file for both stdout and stderr
        let tokio_file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)
            .await
            .with_context(|| format!("Failed to open virtiofsd log file: {}", log_file))?;

        let log_file_handle = tokio_file.into_std().await;

        // Clone for stderr
        let stderr_handle = log_file_handle
            .try_clone()
            .with_context(|| "Failed to clone log file handle for stderr")?;

        cmd.stdout(std::process::Stdio::from(log_file_handle));
        cmd.stderr(std::process::Stdio::from(stderr_handle));

        debug!("virtiofsd output will be logged to: {}", log_file);
    } else {
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
    }

    let child = cmd.spawn().with_context(|| {
        format!(
            "Failed to spawn virtiofsd. Binary: {}, Socket: {}, Shared dir: {}",
            virtiofsd_binary, config.socket_path, config.shared_dir
        )
    })?;

    debug!(
        "Spawned virtiofsd: binary={}, socket={}, shared_dir={}, debug={}, log_file={:?}",
        virtiofsd_binary, config.socket_path, config.shared_dir, config.debug, config.log_file
    );

    Ok(child)
}

/// Validate virtiofsd configuration.
///
/// Checks shared directory exists/readable, socket path valid,
/// and creates socket directory if needed.
pub fn validate_virtiofsd_config(config: &VirtiofsConfig) -> Result<()> {
    // Validate shared directory
    let shared_path = std::path::Path::new(&config.shared_dir);
    if !shared_path.exists() {
        return Err(eyre!(
            "Virtiofsd shared directory does not exist: {}",
            config.shared_dir
        ));
    }

    if !shared_path.is_dir() {
        return Err(eyre!(
            "Virtiofsd shared directory is not a directory: {}",
            config.shared_dir
        ));
    }

    // Check if directory is readable
    match std::fs::read_dir(shared_path) {
        Ok(_) => {}
        Err(e) => {
            return Err(eyre!(
                "Cannot read virtiofsd shared directory {}: {}",
                config.shared_dir,
                e
            ));
        }
    }

    // Validate socket path
    if config.socket_path.as_str().is_empty() {
        return Err(eyre!("Virtiofsd socket path cannot be empty"));
    }

    let socket_path = std::path::Path::new(&config.socket_path);
    if let Some(socket_dir) = socket_path.parent() {
        if !socket_dir.exists() {
            std::fs::create_dir_all(socket_dir).with_context(|| {
                format!(
                    "Failed to create socket directory: {}",
                    socket_dir.display()
                )
            })?;
        }
    }

    Ok(())
}
