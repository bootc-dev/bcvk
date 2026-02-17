//! Anaconda-based bootc installation using kickstart
//!
//! This module implements bootc container installation using anaconda as the
//! installation engine. It uses the ephemeral VM infrastructure to run anaconda
//! in an isolated environment with access to block devices and container storage.
//!
//! Unlike `bcvk to-disk` which runs `bootc install` directly, this approach:
//! - Uses anaconda for partitioning and system configuration via kickstart
//! - Runs anaconda via systemd in the VM (auto-starts on boot, powers off when done)
//! - Uses the `ostreecontainer` kickstart verb with `--transport=containers-storage`
//!
//! ## Example kickstart
//!
//! The user must provide a kickstart file with partitioning, locale, and other
//! system configuration. The `ostreecontainer` directive is **injected automatically**.
//!
//! ```kickstart
//! text
//! lang en_US.UTF-8
//! keyboard us
//! timezone UTC --utc
//! network --bootproto=dhcp --activate
//!
//! zerombr
//! clearpart --all --initlabel
//! autopart --type=plain --fstype=xfs
//! bootloader --location=mbr
//! rootpw --lock
//!
//! poweroff
//! ```
//!
//! bcvk will inject:
//! - `ostreecontainer --transport=containers-storage --url=<image>`
//! - `%post` script to repoint the installed system to the registry image

use camino::Utf8PathBuf;
use clap::Parser;
use color_eyre::eyre::{eyre, Context};
use color_eyre::Result;
use indoc::formatdoc;
use tracing::{debug, info, warn};

use crate::images;
use crate::install_options::InstallOptions;
use crate::run_ephemeral::{CommonVmOpts, RunEphemeralOpts};
use crate::to_disk::Format;
use crate::utils::DiskSize;

const DEFAULT_ANACONDA_IMAGE: &str = "localhost/anaconda-bootc:latest";
const KICKSTART_FILENAME: &str = "anaconda.ks";
const KICKSTART_MOUNT_NAME: &str = "kickstart";
/// Path where kickstart is mounted inside the VM (via virtiofs)
const KICKSTART_MOUNT_PATH: &str = "/run/virtiofs-mnt-kickstart";

/// Minimum disk size for anaconda installations.
///
/// Anaconda requires space for the installed system plus working space for
/// package installation, SELinux relabeling, etc. 4GB is a conservative
/// minimum that works for most base bootc images.
const MIN_DISK_SIZE: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, Parser)]
pub struct AnacondaInstallOpts {
    /// Bootc container image to install (from host container storage)
    pub image: String,

    /// Target disk image file path
    pub target_disk: Utf8PathBuf,

    /// Kickstart file with partitioning and system configuration
    ///
    /// Must contain partitioning (e.g., autopart), locale settings (lang,
    /// keyboard, timezone), and other system configuration. The `ostreecontainer`
    /// directive, and `%post` registry repointing are injected automatically.
    #[clap(long, short = 'k')]
    pub kickstart: std::path::PathBuf,

    /// Target image reference for the installed system
    ///
    /// After installation, the system's bootc origin is repointed to this
    /// registry image so that `bootc upgrade` pulls updates from the registry
    /// rather than expecting containers-storage. Defaults to the image argument.
    #[clap(long)]
    pub target_imgref: Option<String>,

    /// Skip injecting the %post script that repoints to target-imgref
    ///
    /// Use this if you want to handle bootc origin configuration yourself
    /// in your kickstart file.
    #[clap(long)]
    pub no_repoint: bool,

    /// Anaconda container image to use as the installer
    #[clap(long, default_value = DEFAULT_ANACONDA_IMAGE)]
    pub anaconda_image: String,

    /// Disk size to create (e.g. 10G, 5120M)
    #[clap(long)]
    pub disk_size: Option<DiskSize>,

    /// Output disk image format
    #[clap(long, default_value_t = Format::Raw)]
    pub format: Format,

    #[clap(flatten)]
    pub install: InstallOptions,

    #[clap(flatten)]
    pub common: CommonVmOpts,
}

impl AnacondaInstallOpts {
    fn calculate_disk_size(&self) -> Result<u64> {
        if let Some(size) = self.disk_size {
            return Ok(size.as_bytes());
        }
        let image_size = images::get_image_size(&self.image)?;
        Ok(std::cmp::max(image_size * 2, MIN_DISK_SIZE))
    }

    /// Get the target image reference for repointing after installation
    fn get_target_imgref(&self) -> &str {
        self.target_imgref.as_deref().unwrap_or(&self.image)
    }

    /// Validate that an image reference doesn't contain characters that could
    /// inject kickstart or shell syntax.
    fn validate_image_ref(name: &str, field: &str) -> Result<()> {
        if name.contains('\n') || name.contains('%') {
            return Err(eyre!(
                "{} contains invalid characters (newlines or '%' not allowed)",
                field
            ));
        }
        Ok(())
    }

    /// Generate the final kickstart by reading user kickstart and injecting
    /// bcvk-specific directives.
    fn generate_kickstart(&self) -> Result<String> {
        let user_kickstart = std::fs::read_to_string(&self.kickstart)
            .with_context(|| format!("Failed to read kickstart: {}", self.kickstart.display()))?;

        // Validate that user kickstart doesn't contain ostreecontainer directive
        // (we inject that ourselves). Ignore comments.
        for line in user_kickstart.lines() {
            let trimmed = line.trim();
            // Skip comments
            if trimmed.starts_with('#') {
                continue;
            }
            if trimmed.starts_with("ostreecontainer") {
                return Err(eyre!(
                    "Kickstart must not contain 'ostreecontainer' directive; \
                     bcvk injects this automatically with the correct transport"
                ));
            }
        }

        // Validate both image and target_imgref don't contain injection characters
        Self::validate_image_ref(&self.image, "Image name")?;
        if let Some(ref target) = self.target_imgref {
            Self::validate_image_ref(target, "Target image reference (--target-imgref)")?;
        }

        // Build the %post script for repointing to registry
        let post_section = if self.no_repoint {
            String::new()
        } else {
            let target = self.get_target_imgref();
            // Shell-quote the target to prevent command injection
            let quoted_target = shlex::try_quote(target)
                .map_err(|e| eyre!("Target image reference contains invalid characters: {}", e))?;
            formatdoc! {r#"

                %post --erroronfail
                set -euo pipefail
                # Repoint bootc origin to registry so `bootc upgrade` works
                bootc switch --mutate-in-place --transport registry {quoted_target}
                %end
            "#,
                quoted_target = quoted_target,
            }
        };

        // Inject ostreecontainer directive before any %pre/%post sections
        let mut result = String::new();
        let mut ostreecontainer_added = false;

        for line in user_kickstart.lines() {
            let trimmed = line.trim();

            // Detect section boundaries - insert ostreecontainer before first section
            if trimmed.starts_with('%') && !trimmed.starts_with("%%") && !ostreecontainer_added {
                result.push_str(&format!(
                    "ostreecontainer --transport=containers-storage --url={}\n",
                    self.image
                ));
                ostreecontainer_added = true;
            }

            result.push_str(line);
            result.push('\n');
        }

        // If no sections exist, add at the end
        if !ostreecontainer_added {
            result.push_str(&format!(
                "ostreecontainer --transport=containers-storage --url={}\n",
                self.image
            ));
        }

        // Always add our %post at the end (after user's sections)
        result.push_str(&post_section);

        Ok(result)
    }

    fn write_kickstart_to_tempdir(&self) -> Result<(tempfile::TempDir, Utf8PathBuf)> {
        let content = self.generate_kickstart()?;
        let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
        let path = temp_dir.path().join(KICKSTART_FILENAME);

        std::fs::write(&path, &content).context("Failed to write kickstart file")?;

        let path: Utf8PathBuf = path.try_into().context("Temp path is not valid UTF-8")?;
        debug!("Wrote kickstart to: {}", path);
        debug!("Kickstart content:\n{}", content);
        Ok((temp_dir, path))
    }
}

pub fn install(_global_opts: &super::AnacondaOptions, opts: AnacondaInstallOpts) -> Result<()> {
    info!(
        "Installing {} via anaconda ({})",
        opts.image, opts.anaconda_image
    );
    if !opts.no_repoint {
        info!(
            "Target imgref for bootc origin: {}",
            opts.get_target_imgref()
        );
    }

    let disk_size = opts.calculate_disk_size()?;
    let (kickstart_tempdir, _) = opts.write_kickstart_to_tempdir()?;
    let kickstart_dir: Utf8PathBuf = kickstart_tempdir
        .path()
        .to_path_buf()
        .try_into()
        .context("Temp directory path is not valid UTF-8")?;

    info!("Creating target disk: {}", opts.target_disk);
    match opts.format {
        Format::Raw => {
            // Create sparse file - only allocates space as data is written
            let file = std::fs::File::create(&opts.target_disk)
                .with_context(|| format!("Creating {}", opts.target_disk))?;
            file.set_len(disk_size)?;
        }
        Format::Qcow2 => {
            // Use qemu-img to create qcow2 format
            debug!("Creating qcow2 with size {} bytes", disk_size);
            let size_arg = disk_size.to_string();
            let output = std::process::Command::new("qemu-img")
                .args([
                    "create",
                    "-f",
                    "qcow2",
                    opts.target_disk.as_str(),
                    &size_arg,
                ])
                .output()
                .with_context(|| {
                    format!("Failed to run qemu-img create for {}", opts.target_disk)
                })?;

            if !output.status.success() {
                return Err(eyre!(
                    "qemu-img create failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
    }

    // Build ephemeral VM options
    // The anaconda-install.service in the container will auto-start and poweroff when done
    let ephemeral_opts = RunEphemeralOpts {
        host_dns_servers: None,
        image: opts.anaconda_image.clone(),
        common: opts.common.clone(),
        podman: crate::run_ephemeral::CommonPodmanOptions {
            rm: true,
            detach: false, // Wait for completion
            tty: false,
            ..Default::default()
        },
        add_swap: Some(format!("{disk_size}")),
        bind_mounts: Vec::new(),
        ro_bind_mounts: vec![format!("{}:{}", kickstart_dir, KICKSTART_MOUNT_NAME)],
        systemd_units_dir: None,
        bind_storage_ro: true,
        mount_disk_files: vec![format!(
            "{}:output:{}",
            opts.target_disk,
            opts.format.as_str()
        )],
        kernel_args: vec![
            // Use anaconda's direct mode (no tmux)
            "inst.notmux".to_string(),
            // Point to our virtiofs-mounted kickstart
            format!("inst.ks=file://{}/anaconda.ks", KICKSTART_MOUNT_PATH),
            // Marker for bcvk-anaconda-setup.service to activate
            "bcvk.anaconda".to_string(),
        ],
        debug_entrypoint: None,
    };

    info!("Starting anaconda VM (will poweroff when complete)...");

    // Run the ephemeral VM - it will poweroff when anaconda completes
    // Use run_sync to spawn as subprocess and wait, rather than exec which replaces the process
    let result = crate::run_ephemeral::run_sync(ephemeral_opts);

    // Clean up temp directory
    drop(kickstart_tempdir);

    match result {
        Ok(()) => {
            println!("\nInstallation completed successfully!");
            println!("Output disk: {}", opts.target_disk);
            Ok(())
        }
        Err(e) => {
            if let Err(cleanup_err) = std::fs::remove_file(&opts.target_disk) {
                warn!(
                    "Failed to clean up disk image {}: {}",
                    opts.target_disk, cleanup_err
                );
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper to create a test opts struct with a kickstart file
    fn create_test_opts(kickstart_content: &str) -> (TempDir, AnacondaInstallOpts) {
        let temp_dir = TempDir::new().unwrap();
        let ks_path = temp_dir.path().join("test.ks");
        std::fs::write(&ks_path, kickstart_content).unwrap();

        let opts = AnacondaInstallOpts {
            image: "quay.io/fedora/fedora-bootc:42".to_string(),
            target_disk: "/tmp/test.img".into(),
            kickstart: ks_path,
            target_imgref: None,
            no_repoint: false,
            anaconda_image: DEFAULT_ANACONDA_IMAGE.to_string(),
            disk_size: None,
            format: Format::Raw,
            install: InstallOptions::default(),
            common: CommonVmOpts::default(),
        };

        (temp_dir, opts)
    }

    #[test]
    fn test_generate_kickstart_basic() {
        let ks = "text\nlang en_US.UTF-8\npoweroff\n";
        let (_dir, opts) = create_test_opts(ks);

        let result = opts.generate_kickstart().unwrap();

        // Should contain the original content
        assert!(result.contains("text"));
        assert!(result.contains("lang en_US.UTF-8"));
        assert!(result.contains("poweroff"));

        // Should inject ostreecontainer
        assert!(result.contains("ostreecontainer --transport=containers-storage"));
        assert!(result.contains("--url=quay.io/fedora/fedora-bootc:42"));

        // Should inject %post for repointing
        assert!(result.contains("%post --erroronfail"));
        assert!(result.contains("bootc switch --mutate-in-place --transport registry"));
    }

    #[test]
    fn test_generate_kickstart_no_repoint() {
        let ks = "text\npoweroff\n";
        let (_dir, mut opts) = create_test_opts(ks);
        opts.no_repoint = true;

        let result = opts.generate_kickstart().unwrap();

        // Should NOT inject %post
        assert!(!result.contains("%post"));
        assert!(!result.contains("bootc switch"));

        // Should still inject ostreecontainer
        assert!(result.contains("ostreecontainer"));
    }

    #[test]
    fn test_generate_kickstart_with_target_imgref() {
        let ks = "text\npoweroff\n";
        let (_dir, mut opts) = create_test_opts(ks);
        opts.target_imgref = Some("registry.example.com/myapp:prod".to_string());

        let result = opts.generate_kickstart().unwrap();

        // Should use target_imgref in %post, not the source image
        assert!(result.contains("registry.example.com/myapp:prod"));
        // The ostreecontainer should still use the source image
        assert!(result.contains("--url=quay.io/fedora/fedora-bootc:42"));
    }

    #[test]
    fn test_generate_kickstart_ostreecontainer_in_comment_allowed() {
        // Comments mentioning ostreecontainer should be allowed
        let ks = "# Note: don't use ostreecontainer here\ntext\npoweroff\n";
        let (_dir, opts) = create_test_opts(ks);

        let result = opts.generate_kickstart();
        assert!(result.is_ok(), "Should allow ostreecontainer in comments");
    }

    #[test]
    fn test_generate_kickstart_rejects_ostreecontainer_directive() {
        let ks = "text\nostreecontainer --url=foo\npoweroff\n";
        let (_dir, opts) = create_test_opts(ks);

        let result = opts.generate_kickstart();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ostreecontainer"));
    }

    #[test]
    fn test_generate_kickstart_rejects_newline_in_image() {
        let ks = "text\npoweroff\n";
        let (_dir, mut opts) = create_test_opts(ks);
        opts.image = "foo\nbar".to_string();

        let result = opts.generate_kickstart();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("newlines"));
    }

    #[test]
    fn test_generate_kickstart_rejects_percent_in_image() {
        let ks = "text\npoweroff\n";
        let (_dir, mut opts) = create_test_opts(ks);
        opts.image = "foo%bar".to_string();

        let result = opts.generate_kickstart();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("%"));
    }

    #[test]
    fn test_generate_kickstart_rejects_newline_in_target_imgref() {
        let ks = "text\npoweroff\n";
        let (_dir, mut opts) = create_test_opts(ks);
        opts.target_imgref = Some("foo\nbar".to_string());

        let result = opts.generate_kickstart();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("target-imgref"));
    }

    #[test]
    fn test_generate_kickstart_with_existing_post_section() {
        let ks = "text\n%post\necho hello\n%end\npoweroff\n";
        let (_dir, opts) = create_test_opts(ks);

        let result = opts.generate_kickstart().unwrap();

        // Should preserve user's %post
        assert!(result.contains("echo hello"));

        // Should add ostreecontainer BEFORE user's %post
        let ostree_pos = result.find("ostreecontainer").unwrap();
        let user_post_pos = result.find("echo hello").unwrap();
        assert!(
            ostree_pos < user_post_pos,
            "ostreecontainer should be before user's %post"
        );

        // Should add our %post at the end
        let our_post = result.rfind("bootc switch").unwrap();
        assert!(
            our_post > user_post_pos,
            "our %post should be after user's %post"
        );
    }

    #[test]
    fn test_generate_kickstart_shell_quoting() {
        let ks = "text\npoweroff\n";
        let (_dir, mut opts) = create_test_opts(ks);
        // Image with spaces (unusual but valid in some contexts)
        opts.target_imgref = Some("registry.example.com/my app:v1".to_string());

        let result = opts.generate_kickstart().unwrap();

        // Should be properly quoted for shell
        assert!(
            result.contains("'registry.example.com/my app:v1'")
                || result.contains("\"registry.example.com/my app:v1\""),
            "Image ref with spaces should be quoted: {}",
            result
        );
    }
}
