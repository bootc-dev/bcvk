use std::ffi::OsString;
use std::path::Path;

use clap::{Parser, Subcommand};
use color_eyre::{Report, Result};
use tracing::instrument;
use virtinstall::VirtInstallOpts;

pub(crate) mod containerenv;
mod entrypoint;
mod envdetect;
mod hostexec;
mod images;
mod init;
mod runrmvm;
mod sshcred;
mod virtinstall;
mod vm;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Parser)]
struct EntrypointOpts {
    /// Path where to write the entrypoint script
    #[clap(long)]
    output: Option<String>,
}

#[derive(Parser)]
struct HostExecOpts {
    /// Binary to run
    bin: OsString,

    /// Arguments to pass to the binary
    #[clap(allow_hyphen_values = true)]
    args: Vec<OsString>,
}

#[derive(Subcommand)]
enum Commands {
    /// Execute a command in the host context
    Hostexec(HostExecOpts),
    #[clap(subcommand)]
    Images(images::ImagesOpts),
    #[clap(subcommand)]
    VirtInstall(VirtInstallOpts),
    /// Initialize bootc-kit infrastructure
    Init(init::InitOpts),
    /// Run a bootc container in an ephemeral VM
    RunRmVm(runrmvm::RunRmVmOpts),
    /// Generate an entrypoint script
    Entrypoint(EntrypointOpts),
}

fn install_tracing() {
    use tracing_error::ErrorLayer;
    use tracing_subscriber::fmt;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::EnvFilter;

    let fmt_layer = fmt::layer().with_target(false);
    let filter_layer = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap();

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt_layer)
        .with(ErrorLayer::default())
        .init();
}

#[instrument]
fn main() -> Result<(), Report> {
    install_tracing();
    color_eyre::install()?;

    let cli = Cli::parse();

    match cli.command {
        Commands::Hostexec(opts) => {
            hostexec::run(opts.bin, opts.args)?;
        }
        Commands::Images(opts) => opts.run()?,
        Commands::VirtInstall(opts) => opts.run()?,
        Commands::Init(opts) => opts.run()?,
        Commands::RunRmVm(opts) => opts.run()?,
        Commands::Entrypoint(opts) => {
            if let Some(path) = opts.output {
                entrypoint::generate_entrypoint_script(Path::new(&path))?;
            } else {
                entrypoint::print_entrypoint_script()?;
            }
        },
    }
    Ok(())
}
