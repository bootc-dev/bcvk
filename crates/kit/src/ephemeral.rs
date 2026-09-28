//! Ephemeral VM management commands
//!
//! This module provides subcommands for running bootc containers as ephemeral virtual machines.
//! Ephemeral VMs are temporary, non-persistent VMs that are useful for testing, development,
//! and CI/CD workflows.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::Command;

use clap::Subcommand;
use color_eyre::{
    eyre::{eyre, Context as _},
    Result,
};
use comfy_table::{presets::UTF8_FULL, Table};
use serde::{Deserialize, Serialize};

// Re-export the existing implementations
use crate::run_ephemeral;
use crate::run_ephemeral_ssh;
use crate::ssh;

/// Label used to identify bcvk ephemeral containers
const EPHEMERAL_LABEL: &str = "bcvk.ephemeral=1";

/// Name of the `test-basic` subcommand, which re-executes itself as `run-ssh`
const TEST_BASIC_CMD: &str = "test-basic";

/// How long `test-basic` waits for boot to finish once SSH is up. Without a
/// bound, a start job with no timeout (the default for `Type=oneshot`) would
/// hang it forever.
const TEST_BASIC_TIMEOUT_SECS: u32 = 600;

/// Exit status of `test-basic` when boot didn't finish in time (that of
/// timeout(1)); it exits 1 when systemd finished in a state other than
/// "running".
const TEST_BASIC_TIMEOUT_EXIT: u8 = 124;

/// Command run in the guest by `test-basic`: wait for boot to finish, and if
/// systemd didn't reach "running" (e.g. "degraded"), show the failed units,
/// or the pending jobs if it timed out.
fn test_basic_script() -> String {
    let secs = TEST_BASIC_TIMEOUT_SECS;
    let timeout_rc = TEST_BASIC_TIMEOUT_EXIT;
    format!(
        "timeout {secs} systemctl is-system-running --wait; rc=$?; \
         if [ $rc -eq {timeout_rc} ]; then \
         echo 'Timed out after {secs}s waiting for boot to finish; pending jobs:' >&2; \
         systemctl list-jobs --no-pager >&2; exit {timeout_rc}; \
         elif [ $rc -ne 0 ]; then systemctl --failed --no-pager; exit 1; fi"
    )
}

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

/// Options for the test-basic subcommand
#[derive(clap::Parser, Debug)]
pub struct TestBasicOpts {
    #[command(flatten)]
    pub run_opts: run_ephemeral::RunEphemeralOpts,
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

    /// Boot an ephemeral VM and check that systemd reaches the running state
    ///
    /// A smoke test for bootc images: this is shorthand for
    /// `bcvk ephemeral run-ssh IMAGE -- systemctl is-system-running --wait`,
    /// which also lists the failed units when the system comes up degraded.
    /// On success it prints just the state, "running". Otherwise it exits 1,
    /// or 124 if boot didn't finish within 10 minutes of SSH coming up.
    #[clap(name = TEST_BASIC_CMD)]
    TestBasic(TestBasicOpts),

    /// Connect to running VMs via SSH
    #[clap(name = "ssh")]
    Ssh(SshOpts),

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
            // The options were parsed only to validate them (and for --help);
            // run-ssh takes the same ones, straight from our argv.
            EphemeralCommands::TestBasic(_) => test_basic(),
            EphemeralCommands::Ssh(opts) => {
                // Create progress bar if stderr is a terminal
                let progress_bar = crate::boot_progress::create_boot_progress_bar();

                run_ephemeral_ssh::wait_for_ssh_ready(&opts.container_name, None, progress_bar)?;

                ssh::connect_via_container(&opts.container_name, opts.args)
            }
            EphemeralCommands::Ps { json } => {
                let containers = list_ephemeral_containers()?;

                if json {
                    let json_output = serde_json::to_string_pretty(&containers)?;
                    println!("{}", json_output);
                } else {
                    // Create a table using comfy_table
                    let mut table = Table::new();
                    table.load_style(UTF8_FULL).set_header(vec![
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

/// Arguments for `bcvk ephemeral run-ssh` equivalent to our own `test-basic`
/// invocation, given its arguments (without argv\[0\]).
fn test_basic_run_ssh_args(args: impl IntoIterator<Item = OsString>) -> Result<Vec<OsString>> {
    let mut args = args.into_iter().skip_while(|arg| arg != TEST_BASIC_CMD);
    if args.next().is_none() {
        return Err(eyre!("Failed to find {TEST_BASIC_CMD} in arguments"));
    }
    let opts: Vec<OsString> = args.collect();
    let mut run_ssh_args: Vec<OsString> = ["ephemeral", "run-ssh"].map(Into::into).into();
    // The image is the only positional argument, so a `--` the user already
    // gave before it also separates the command from it.
    let has_separator = opts.iter().any(|arg| arg == "--");
    run_ssh_args.extend(opts);
    if !has_separator {
        run_ssh_args.push("--".into());
    }
    run_ssh_args.extend(["/bin/sh".into(), "-c".into(), test_basic_script().into()]);
    Ok(run_ssh_args)
}

/// Run `test-basic` by re-executing ourselves as `run-ssh`.
fn test_basic() -> Result<()> {
    let mut argv = std::env::args_os();
    let arg0 = argv.next().unwrap_or_else(|| "bcvk".into());
    let args = test_basic_run_ssh_args(argv)?;
    let err = Command::new("/proc/self/exe").arg0(arg0).args(args).exec();
    Err(err).context("Failed to execute bcvk ephemeral run-ssh")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_args_to_run_ssh() {
        let script = test_basic_script();
        let command = ["/bin/sh", "-c", script.as_str()];
        let cases: &[(&[&str], &[&str])] = &[
            (
                &["ephemeral", "test-basic", "quay.io/example/os"],
                &["quay.io/example/os", "--"],
            ),
            (
                &[
                    "ephemeral",
                    "test-basic",
                    "--memory",
                    "4G",
                    "--label",
                    "k=v",
                    "img",
                ],
                &["--memory", "4G", "--label", "k=v", "img", "--"],
            ),
            (
                &[
                    "ephemeral",
                    "test-basic",
                    "--memory=8G",
                    "-K",
                    "-e",
                    "A=B",
                    "img",
                ],
                &["--memory=8G", "-K", "-e", "A=B", "img", "--"],
            ),
            // A separator the user already gave is reused
            (&["ephemeral", "test-basic", "img", "--"], &["img", "--"]),
            (
                &["ephemeral", "test-basic", "--vcpus", "2", "--", "img"],
                &["--vcpus", "2", "--", "img"],
            ),
        ];
        for (input, expected_opts) in cases {
            let expected: Vec<OsString> = ["ephemeral", "run-ssh"]
                .iter()
                .chain(expected_opts.iter())
                .chain(command.iter())
                .map(Into::into)
                .collect();
            let args = test_basic_run_ssh_args(input.iter().map(Into::into)).unwrap();
            assert_eq!(args, expected, "input: {input:?}");
        }

        assert!(test_basic_run_ssh_args(["ephemeral", "run-ssh"].map(Into::into)).is_err());
    }
}
