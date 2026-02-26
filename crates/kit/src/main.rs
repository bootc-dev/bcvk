//! Bootc Virtualization Kit (bcvk) - A toolkit for bootc containers and local virtualization

use clap::{Parser, Subcommand};
#[cfg(target_os = "linux")]
use color_eyre::eyre::Context as _;
use color_eyre::{Report, Result};

mod cli_json;
mod common_opts;
mod cpio;
mod credentials;
mod install_options;
mod instancetypes;
mod qemu_img;
mod xml_utils;

#[cfg(target_os = "linux")]
mod arch;

#[cfg(target_os = "linux")]
mod images;

// Linux-only modules
#[cfg(target_os = "linux")]
mod boot_progress;
#[cfg(target_os = "linux")]
mod cache_metadata;
#[cfg(target_os = "linux")]
mod container_entrypoint;
#[cfg(target_os = "linux")]
mod domain_list;
#[cfg(target_os = "linux")]
mod ephemeral;
#[cfg(target_os = "linux")]
mod kernel;
#[cfg(target_os = "linux")]
mod libvirt;
#[cfg(target_os = "linux")]
mod libvirt_upload_disk;
#[cfg(target_os = "linux")]
#[allow(dead_code)]
mod podman;
#[cfg(target_os = "linux")]
mod qemu;
#[cfg(target_os = "linux")]
mod run_ephemeral;
#[cfg(target_os = "linux")]
mod run_ephemeral_ssh;
#[cfg(target_os = "linux")]
mod ssh;
#[cfg(target_os = "linux")]
mod status_monitor;
#[cfg(target_os = "linux")]
mod supervisor_status;
#[cfg(target_os = "linux")]
pub(crate) mod systemd;
#[cfg(target_os = "linux")]
mod to_disk;
#[cfg(target_os = "linux")]
mod utils;

/// Default state directory for bcvk container data
#[cfg(target_os = "linux")]
pub const CONTAINER_STATEDIR: &str = "/var/lib/bcvk";

/// A comprehensive toolkit for bootc containers and local virtualization.
///
/// bcvk provides a complete workflow for building, testing, and managing
/// bootc containers using ephemeral VMs. Run bootc images as temporary VMs,
/// install them to disk, or manage existing installations - all without
/// requiring root privileges.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[cfg(target_os = "linux")]
#[derive(Parser)]
struct DebugInternalsOpts {
    #[command(subcommand)]
    command: DebugInternalsCmds,
}

#[cfg(target_os = "linux")]
#[derive(Subcommand)]
enum DebugInternalsCmds {
    OpenTree { path: std::path::PathBuf },
}

/// Internal diagnostic and tooling commands for development
#[derive(Parser)]
struct InternalsOpts {
    #[command(subcommand)]
    command: InternalsCmds,
}

#[derive(Subcommand)]
enum InternalsCmds {
    /// Dump CLI structure as JSON for man page generation
    #[cfg(feature = "docgen")]
    DumpCliJson,
}

/// Stub subcommands for macOS (shows error message when run)
#[cfg(not(target_os = "linux"))]
#[derive(Debug, Subcommand)]
pub enum StubEphemeralCommands {
    /// Run bootc containers as ephemeral VMs
    #[clap(name = "run")]
    Run,
    /// Run ephemeral VM and SSH into it
    #[clap(name = "run-ssh")]
    RunSsh,
    /// Connect to running VMs via SSH
    #[clap(name = "ssh")]
    Ssh,
    /// List ephemeral VM containers
    #[clap(name = "ps")]
    Ps,
    /// Remove all ephemeral VM containers
    #[clap(name = "rm-all")]
    RmAll,
}

/// Available bcvk commands for container and VM management.
#[derive(Subcommand)]
enum Commands {
    // Linux-only commands with full functionality
    #[cfg(target_os = "linux")]
    /// Manage and inspect bootc container images
    #[clap(subcommand)]
    Images(images::ImagesOpts),

    #[cfg(target_os = "linux")]
    /// Manage ephemeral VMs for bootc containers
    #[clap(subcommand)]
    Ephemeral(ephemeral::EphemeralCommands),

    // macOS stub: ephemeral command exists but errors out
    #[cfg(not(target_os = "linux"))]
    /// Manage ephemeral VMs for bootc containers (not available on this platform)
    #[clap(subcommand)]
    Ephemeral(StubEphemeralCommands),

    #[cfg(target_os = "linux")]
    /// Install bootc images to persistent disk images
    #[clap(name = "to-disk")]
    ToDisk(to_disk::ToDiskOpts),

    // Note: libvirt is intentionally NOT available on macOS
    #[cfg(target_os = "linux")]
    /// Manage libvirt integration for bootc containers
    Libvirt {
        /// Hypervisor connection URI (e.g., qemu:///system, qemu+ssh://host/system)
        #[clap(short = 'c', long = "connect", global = true)]
        connect: Option<String>,

        #[command(subcommand)]
        command: libvirt::LibvirtSubcommands,
    },

    #[cfg(target_os = "linux")]
    /// Upload bootc disk images to libvirt (deprecated)
    #[clap(name = "libvirt-upload-disk", hide = true)]
    LibvirtUploadDisk(libvirt_upload_disk::LibvirtUploadDiskOpts),

    #[cfg(target_os = "linux")]
    /// Internal container entrypoint command (hidden from help)
    #[clap(hide = true)]
    ContainerEntrypoint(container_entrypoint::ContainerEntrypointOpts),

    #[cfg(target_os = "linux")]
    /// Internal debugging and diagnostic tools (hidden from help)
    #[clap(hide = true)]
    DebugInternals(DebugInternalsOpts),

    /// Internal diagnostic and tooling commands for development
    #[clap(hide = true)]
    Internals(InternalsOpts),
}

/// Install and configure the tracing/logging system.
///
/// Sets up structured logging with environment-based filtering,
/// error layer integration, and console output formatting.
/// Logs are filtered by RUST_LOG environment variable, defaulting to 'info'.
fn install_tracing() {
    use tracing_error::ErrorLayer;
    use tracing_subscriber::fmt;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::EnvFilter;

    let format = fmt::format().without_time().with_target(false).compact();

    let fmt_layer = fmt::layer()
        .event_format(format)
        .with_writer(std::io::stderr);
    let filter_layer = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap();

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt_layer)
        .with(ErrorLayer::default())
        .init();
}

/// Main entry point for the bcvk CLI application.
///
/// Initializes logging, error handling, and command dispatch for all
/// bcvk operations including VM management, SSH access, and
/// container image handling.
// On non-Linux, all commands return errors so post-match code is unreachable
#[cfg_attr(not(target_os = "linux"), allow(unreachable_code))]
fn main() -> Result<(), Report> {
    install_tracing();
    color_eyre::install()?;

    let cli = Cli::parse();

    #[cfg(target_os = "linux")]
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Init tokio runtime")?;

    match cli.command {
        #[cfg(target_os = "linux")]
        Commands::Images(opts) => opts.run()?,

        #[cfg(target_os = "linux")]
        Commands::Ephemeral(cmd) => cmd.run()?,

        // macOS stub: ephemeral command exists but errors out
        #[cfg(not(target_os = "linux"))]
        Commands::Ephemeral(_) => {
            return Err(color_eyre::eyre::eyre!(
                "The 'ephemeral' command is not available on macOS.\n\
                 bcvk requires Linux with KVM/QEMU for VM operations.\n\
                 See https://github.com/bootc-dev/bcvk/issues/21 for more information."
            ));
        }

        #[cfg(target_os = "linux")]
        Commands::ToDisk(opts) => {
            to_disk::run(opts)?;
        }

        #[cfg(target_os = "linux")]
        Commands::Libvirt { connect, command } => {
            let options = libvirt::LibvirtOptions { connect };
            match command {
                libvirt::LibvirtSubcommands::Run(opts) => libvirt::run::run(&options, opts)?,
                libvirt::LibvirtSubcommands::Ssh(opts) => libvirt::ssh::run(&options, opts)?,
                libvirt::LibvirtSubcommands::List(opts) => libvirt::list::run(&options, opts)?,
                libvirt::LibvirtSubcommands::ListVolumes(opts) => {
                    libvirt::list_volumes::run(&options, opts)?
                }
                libvirt::LibvirtSubcommands::Stop(opts) => libvirt::stop::run(&options, opts)?,
                libvirt::LibvirtSubcommands::Start(opts) => libvirt::start::run(&options, opts)?,
                libvirt::LibvirtSubcommands::Remove(opts) => libvirt::rm::run(&options, opts)?,
                libvirt::LibvirtSubcommands::RemoveAll(opts) => {
                    libvirt::rm_all::run(&options, opts)?
                }
                libvirt::LibvirtSubcommands::Inspect(opts) => {
                    libvirt::inspect::run(&options, opts)?
                }
                libvirt::LibvirtSubcommands::Upload(opts) => libvirt::upload::run(&options, opts)?,
                libvirt::LibvirtSubcommands::Status(opts) => libvirt::status::run(opts)?,
                libvirt::LibvirtSubcommands::BaseDisks(opts) => {
                    libvirt::base_disks_cli::run(&options, opts)?
                }
                libvirt::LibvirtSubcommands::PrintFirmware(opts) => {
                    libvirt::print_firmware::run(opts)?
                }
            }
        }

        #[cfg(target_os = "linux")]
        Commands::LibvirtUploadDisk(opts) => {
            eprintln!(
                "Warning: 'libvirt-upload-disk' is deprecated. Use 'libvirt upload' instead."
            );
            libvirt_upload_disk::run(opts)?;
        }

        #[cfg(target_os = "linux")]
        Commands::ContainerEntrypoint(opts) => {
            // Create a tokio runtime for async container entrypoint operations
            rt.block_on(async move {
                let r = container_entrypoint::run(opts).await;
                tracing::debug!("Container entrypoint done");
                r
            })?;
            tracing::trace!("Exiting runtime");
        }

        #[cfg(target_os = "linux")]
        Commands::DebugInternals(opts) => match opts.command {
            DebugInternalsCmds::OpenTree { path } => {
                use cap_std_ext::cap_std::fs::Dir;
                let fd = rustix::mount::open_tree(
                    rustix::fs::CWD,
                    path,
                    rustix::mount::OpenTreeFlags::OPEN_TREE_CLOEXEC
                        | rustix::mount::OpenTreeFlags::OPEN_TREE_CLONE,
                )?;
                let fd = Dir::reopen_dir(&fd)?;
                tracing::debug!("{:?}", fd.entries()?.into_iter().collect::<Vec<_>>());
            }
        },

        Commands::Internals(opts) => {
            #[cfg(feature = "docgen")]
            match opts.command {
                InternalsCmds::DumpCliJson => {
                    let json = cli_json::dump_cli_json()?;
                    println!("{}", json);
                }
            }

            // Without docgen feature, Internals has no subcommands
            #[cfg(not(feature = "docgen"))]
            {
                let _ = opts;
                return Err(color_eyre::eyre::eyre!(
                    "No internal commands available without docgen feature"
                ));
            }
        }
    }

    tracing::debug!("exiting");

    // Ensure we don't block on any spawned tasks
    #[cfg(target_os = "linux")]
    rt.shutdown_background();

    std::process::exit(0)
}
