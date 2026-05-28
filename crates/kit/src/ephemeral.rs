//! Ephemeral VM management commands
//!
//! This module provides subcommands for running bootc containers as ephemeral virtual machines.
//! Ephemeral VMs are temporary, non-persistent VMs that are useful for testing, development,
//! and CI/CD workflows.

use std::process::Command;

use clap::Subcommand;
use color_eyre::{eyre::eyre, Result};
use comfy_table::{presets::UTF8_FULL, Table};
use serde::{Deserialize, Serialize};

// Re-export the existing implementations
use crate::run_ephemeral;
use crate::run_ephemeral_ssh;
use crate::ssh;

/// Label used to identify bcvk ephemeral containers
const EPHEMERAL_LABEL: &str = "bcvk.ephemeral=1";

/// SSH connection options for accessing running VMs.
///
/// Provides secure shell access to VMs running within containers,
/// with automatic key management and connection routing.
#[derive(clap::Parser, Debug)]
pub struct SshOpts {
    /// Name or ID of the container running the target VM
    ///
    /// This should match the container name from podman or the VM ID
    /// used when starting the ephemeral VM.
    pub container_name: String,

    /// Additional SSH client arguments to pass through
    ///
    /// Standard ssh arguments like -v for verbose output, -L for
    /// port forwarding, or -o for SSH options.
    #[clap(allow_hyphen_values = true, help = "SSH arguments like -v, -L, -o")]
    pub args: Vec<String>,
}

/// Configuration options for SCP file transfer to/from an ephemeral VM
#[derive(clap::Parser, Debug)]
pub struct EphemeralScpOpts {
    /// Name or ID of the container running the target VM
    pub container_name: String,

    /// Source path (use DOMAIN: prefix for remote paths, e.g. `/local/file` or `DOMAIN:/remote/file`)
    pub source: String,

    /// Destination path (use DOMAIN: prefix for remote paths, e.g. `/local/file` or `DOMAIN:/remote/file`)
    pub destination: String,

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

impl EphemeralScpOpts {
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

    /// Build a pre-configured `podman exec ... scp` command
    fn build_podman_scp_command(&self, parsed_extra_options: &[(String, String)]) -> Command {
        let mut scp_cmd = Command::new("podman");
        scp_cmd.args([
            "exec",
            "--",
            &self.container_name,
            "scp",
            "-i",
            &crate::ssh::container_ssh_key_path(),
            "-P",
            &crate::ssh::CONTAINER_SSH_PORT.to_string(),
        ]);

        if self.recursive {
            scp_cmd.arg("-r");
        }

        let common_opts = crate::ssh::CommonSshOptions {
            strict_host_keys: self.strict_host_keys,
            connect_timeout: self.timeout,
            server_alive_interval: crate::libvirt::ssh::SSH_SERVER_ALIVE_INTERVAL,
            log_level: self.log_level.clone(),
            extra_options: parsed_extra_options.to_vec(),
        };
        common_opts.apply_to_command(&mut scp_cmd);

        scp_cmd
    }
}

/// Container list entry for ephemeral VMs
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerListEntry {
    /// Container ID
    pub id: String,

    /// Container names
    pub names: Vec<String>,

    /// Container state
    pub state: String,

    /// Creation timestamp
    pub created_at: String,

    /// Container image
    pub image: String,

    /// Container command
    pub command: Vec<String>,
}

/// Ephemeral VM operations
#[derive(Debug, Subcommand)]
#[command(after_long_help = "\
# Basic usage

  Fire-and-forget interactive session (VM is removed on exit):

    bcvk ephemeral run-ssh quay.io/fedora/fedora-bootc:42

  Background VM you can reconnect to:

    bcvk ephemeral run -d --rm --ssh-keygen --name myvm quay.io/fedora/fedora-bootc:42
    bcvk ephemeral ssh myvm
    podman stop myvm

  Run a single command and capture its exit code (CI pattern):

    bcvk ephemeral run-ssh quay.io/fedora/fedora-bootc:42 -- systemctl is-active myservice

  Stream the boot console in real time:

    bcvk ephemeral run -d --console --name myvm quay.io/fedora/fedora-bootc:42
    podman logs -f myvm

# Custom container images (Containerfile)

  Any image built from a bootc-compatible base can be used directly — no push
  to a registry required.  Build locally and pass the image tag just like a
  public reference:

    podman build -t myimage .
    bcvk ephemeral run-ssh myimage

# Host directory mounts

  Mount a host directory into the VM (available at /run/virtiofs-mnt-src):

    bcvk ephemeral run-ssh --bind .:src quay.io/fedora/fedora-bootc:42

# Disk image inspection (virtio-blk)

  Attach an existing disk image (e.g. a bootc-generated one) as a virtio-blk
  device for mounting, inspection, or fsck without booting that image:

    bcvk ephemeral run-ssh --mount-disk-file /path/to/disk.img:data quay.io/fedora/fedora-bootc:42 -- \\
        sh -c 'mount /dev/disk/by-id/virtio-data-part3 /mnt && ls /mnt'

  The disk appears inside the VM as /dev/disk/by-id/virtio-<name> with
  partition symlinks virtio-<name>-part1 etc.  Bootc images use a GPT layout
  (part1=BIOS-BOOT, part2=EFI, part3=root), so -part3 is the root filesystem.
  Using the virtio-<name> prefix is unambiguous even when multiple disks are
  attached.

# Additional tips

  Make the root filesystem writable (changes are still lost on shutdown):

    bcvk ephemeral run-ssh --karg systemd.volatile=overlay quay.io/fedora/fedora-bootc:42

  Detect ephemeral vs. real hardware in a systemd unit:

    ConditionKernelCommandLine=!rootfstype=virtiofs

  (virtiofs root is the stable indicator that the VM is running under bcvk ephemeral)\
")]
pub enum EphemeralCommands {
    /// Run bootc containers as ephemeral VMs
    #[clap(name = "run")]
    Run(run_ephemeral::RunEphemeralOpts),

    /// Run ephemeral VM and SSH into it
    #[clap(name = "run-ssh")]
    RunSsh(run_ephemeral_ssh::RunEphemeralSshOpts),

    /// Connect to running VMs via SSH
    #[clap(name = "ssh")]
    Ssh(SshOpts),

    /// Copy files to/from an ephemeral VM via SCP
    #[clap(name = "scp")]
    Scp(EphemeralScpOpts),

    /// List ephemeral VM containers
    #[clap(name = "ps")]
    Ps {
        /// Output as structured JSON instead of table format
        #[clap(long)]
        json: bool,
    },

    /// Remove all ephemeral VM containers
    #[clap(name = "rm-all")]
    RmAll {
        /// Force removal without confirmation
        #[clap(short, long)]
        force: bool,
    },
}

impl EphemeralCommands {
    /// Execute the ephemeral subcommand
    pub fn run(self) -> Result<()> {
        match self {
            EphemeralCommands::Run(opts) => run_ephemeral::run(opts),
            EphemeralCommands::RunSsh(opts) => run_ephemeral_ssh::run_ephemeral_ssh(opts),
            EphemeralCommands::Ssh(opts) => {
                // Create progress bar if stderr is a terminal
                let progress_bar = crate::boot_progress::create_boot_progress_bar();

                run_ephemeral_ssh::wait_for_ssh_ready(&opts.container_name, None, progress_bar)?;

                ssh::connect_via_container(&opts.container_name, opts.args)
            }
            EphemeralCommands::Scp(opts) => {
                let source_is_remote = opts.source.starts_with("DOMAIN:");
                let dest_is_remote = opts.destination.starts_with("DOMAIN:");

                if source_is_remote == dest_is_remote {
                    return Err(eyre!(
                        "Exactly one of source or destination must use the DOMAIN: prefix to reference the remote VM.\n\
                         Examples:\n  \
                           bcvk ephemeral scp myvm DOMAIN:/etc/hostname ./hostname\n  \
                           bcvk ephemeral scp myvm ./file.txt DOMAIN:/tmp/file.txt"
                    ));
                }

                let progress_bar = crate::boot_progress::create_boot_progress_bar();
                let (_, progress_bar) = run_ephemeral_ssh::wait_for_ssh_ready(
                    &opts.container_name,
                    None,
                    progress_bar,
                )?;
                progress_bar.finish_and_clear();
                run_ephemeral_scp(opts)
            }
            EphemeralCommands::Ps { json } => {
                let containers = list_ephemeral_containers()?;

                if json {
                    let json_output = serde_json::to_string_pretty(&containers)?;
                    println!("{}", json_output);
                } else {
                    // Create a table using comfy_table
                    let mut table = Table::new();
                    table.load_preset(UTF8_FULL).set_header(vec![
                        "CONTAINER ID",
                        "IMAGE",
                        "CREATED",
                        "STATUS",
                        "NAMES",
                    ]);

                    for container in containers {
                        let id = if container.id.len() > 12 {
                            &container.id[..12]
                        } else {
                            &container.id
                        };

                        let names = container.names.join(", ");
                        let image = if container.image.len() > 30 {
                            format!("{}...", &container.image[..30])
                        } else {
                            container.image.clone()
                        };

                        table.add_row(vec![
                            id.to_string(),
                            image,
                            container.created_at,
                            container.state,
                            names,
                        ]);
                    }

                    println!("{}", table);
                }
                Ok(())
            }
            EphemeralCommands::RmAll { force } => remove_all_ephemeral_containers(force),
        }
    }
}

/// List ephemeral VM containers with bcvk.ephemeral=1 label
pub(crate) fn list_ephemeral_containers() -> Result<Vec<ContainerListEntry>> {
    use bootc_utils::CommandRunExt;

    let containers: Vec<ContainerListEntry> = Command::new("podman")
        .args([
            "ps",
            "--all",
            "--format",
            "json",
            &format!("--filter=label={}", EPHEMERAL_LABEL),
        ])
        .run_and_parse_json()
        .map_err(|e| eyre!("Failed to list ephemeral containers: {}", e))?;
    Ok(containers)
}

/// Per-container result from a removal operation
#[derive(Debug)]
pub(crate) struct RemoveContainerResult {
    /// Container ID that was targeted for removal
    pub id: String,
    /// Whether the container was successfully removed
    pub removed: bool,
    /// Error message if removal failed
    pub error: Option<String>,
}

/// Remove a single container by ID, returning the result.
///
/// Runs `podman rm -f` for the given container ID. This is the building
/// block used by both the CLI (`rm-all`) and the varlink `Rm` method.
pub(crate) fn remove_single_container(container_id: &str) -> RemoveContainerResult {
    let result = Command::new("podman")
        .args(["rm", "-f", "--", container_id])
        .output();
    match result {
        Ok(output) if output.status.success() => RemoveContainerResult {
            id: container_id.to_owned(),
            removed: true,
            error: None,
        },
        Ok(output) => RemoveContainerResult {
            id: container_id.to_owned(),
            removed: false,
            error: Some(String::from_utf8_lossy(&output.stderr).to_string()),
        },
        Err(e) => RemoveContainerResult {
            id: container_id.to_owned(),
            removed: false,
            error: Some(e.to_string()),
        },
    }
}

/// Remove the given ephemeral containers, returning per-container results
pub(crate) fn remove_ephemeral_containers(
    containers: &[ContainerListEntry],
) -> Vec<RemoveContainerResult> {
    containers
        .iter()
        .map(|container| remove_single_container(&container.id))
        .collect()
}

/// Remove all ephemeral VM containers
fn remove_all_ephemeral_containers(force: bool) -> Result<()> {
    let containers = list_ephemeral_containers()?;

    if containers.is_empty() {
        println!("No ephemeral containers found.");
        return Ok(());
    }

    if !force {
        println!("Found {} ephemeral container(s):", containers.len());
        for container in &containers {
            let id = if container.id.len() > 12 {
                &container.id[..12]
            } else {
                &container.id
            };
            let names = container.names.join(", ");
            println!("  {} ({})", id, names);
        }

        print!("Remove all ephemeral containers? [y/N]: ");
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            println!("Aborted.");
            return Ok(());
        }
    }

    let results = remove_ephemeral_containers(&containers);
    for result in &results {
        let short_id = &result.id[..12.min(result.id.len())];
        if result.removed {
            println!("Removed {short_id}");
        } else {
            eprintln!(
                "Failed to remove {}: {}",
                short_id,
                result.error.as_deref().unwrap_or("unknown error")
            );
        }
    }

    Ok(())
}

/// RAII cleanup guard for temporary directory inside container
struct ContainerTempCleanup {
    container_name: String,
    temp_dir: String,
}

impl Drop for ContainerTempCleanup {
    fn drop(&mut self) {
        tracing::debug!("Cleaning up ephemeral SCP temp dir: {}", self.temp_dir);
        let _ = Command::new("podman")
            .args([
                "exec",
                "--",
                &self.container_name,
                "rm",
                "-rf",
                &self.temp_dir,
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// Execute the ephemeral SCP command
pub fn run_ephemeral_scp(opts: EphemeralScpOpts) -> Result<()> {
    tracing::debug!(
        "SCP file transfer for ephemeral container: {}",
        opts.container_name
    );

    // Validate that exactly one of the source or destination starts with "DOMAIN:"
    let source_is_remote = opts.source.starts_with("DOMAIN:");
    let dest_is_remote = opts.destination.starts_with("DOMAIN:");

    if source_is_remote == dest_is_remote {
        return Err(eyre!(
            "Exactly one of source or destination must use the DOMAIN: prefix to reference the remote VM.\n\
             Examples:\n  \
               bcvk ephemeral scp myvm DOMAIN:/etc/hostname ./hostname\n  \
               bcvk ephemeral scp myvm ./file.txt DOMAIN:/tmp/file.txt"
        ));
    }

    let parsed_extra_options = opts.parse_extra_options()?;

    // Generate a unique temporary directory name inside the container
    let temp_dir_name = format!("/tmp/bcvk-scp-{}", uuid::Uuid::new_v4());

    // Make sure the parent directory exists inside the container
    let mkdir_status = Command::new("podman")
        .args([
            "exec",
            "--",
            &opts.container_name,
            "mkdir",
            "-p",
            &temp_dir_name,
        ])
        .status()
        .map_err(|e| eyre!("Failed to execute podman exec mkdir: {}", e))?;

    if !mkdir_status.success() {
        return Err(eyre!(
            "Failed to create temporary directory inside container"
        ));
    }

    // Set up the RAII cleanup guard for the temporary directory
    let _cleanup = ContainerTempCleanup {
        container_name: opts.container_name.clone(),
        temp_dir: temp_dir_name.clone(),
    };

    if dest_is_remote {
        // Uploading
        // Get the filename from the source path
        let file_name = std::path::Path::new(&opts.source)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("transfer");

        let temp_transfer_path = format!("{}/{}", temp_dir_name, file_name);

        // Run podman cp <source> <container_name>:<temp_transfer_path> to copy from the host into the container.
        let cp_status = Command::new("podman")
            .args([
                "cp",
                &opts.source,
                &format!("{}:{}", opts.container_name, temp_transfer_path),
            ])
            .status()
            .map_err(|e| eyre!("Failed to execute podman cp: {}", e))?;

        if !cp_status.success() {
            return Err(eyre!("Failed to copy source file into container"));
        }

        // Run podman exec <container_name> scp -i <key> -P <port> ...
        // to transfer from container temp directory to root@127.0.0.1:<dest_path>
        let dest_path = opts.destination.strip_prefix("DOMAIN:").unwrap();
        let remote_dest = format!("root@127.0.0.1:{}", dest_path);

        let mut scp_cmd = opts.build_podman_scp_command(&parsed_extra_options);
        scp_cmd.arg(&temp_transfer_path);
        scp_cmd.arg(&remote_dest);

        let scp_status = scp_cmd
            .status()
            .map_err(|e| eyre!("Failed to execute scp inside container: {}", e))?;

        if !scp_status.success() {
            return Err(eyre!("SCP upload inside container failed"));
        }
    } else {
        // Downloading
        // Get the filename from the remote source path
        let remote_src_path = opts.source.strip_prefix("DOMAIN:").unwrap();
        let file_name = std::path::Path::new(remote_src_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("transfer");

        let temp_transfer_path = format!("{}/{}", temp_dir_name, file_name);
        let remote_source = format!("root@127.0.0.1:{}", remote_src_path);

        // Run podman exec <container_name> scp -i <key> -P <port> ...
        // to transfer from root@127.0.0.1:<src_path> to temp_transfer_path
        let mut scp_cmd = opts.build_podman_scp_command(&parsed_extra_options);
        scp_cmd.arg(&remote_source);
        scp_cmd.arg(&temp_transfer_path);

        let scp_status = scp_cmd
            .status()
            .map_err(|e| eyre!("Failed to execute scp inside container: {}", e))?;

        if !scp_status.success() {
            return Err(eyre!("SCP download inside container failed"));
        }

        // Run podman cp <container_name>:<temp_transfer_path> <destination> to copy from the container to the host.
        let cp_status = Command::new("podman")
            .args([
                "cp",
                &format!("{}:{}", opts.container_name, temp_transfer_path),
                &opts.destination,
            ])
            .status()
            .map_err(|e| eyre!("Failed to execute podman cp: {}", e))?;

        if !cp_status.success() {
            return Err(eyre!("Failed to copy source file from container to host"));
        }
    }

    Ok(())
}
