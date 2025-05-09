use std::ffi::OsString;

use clap::{Parser, Subcommand};
use color_eyre::{Report, Result};
use tracing::instrument;

pub(crate) mod containerenv;
mod envdetect;
mod hostexec;
mod vm;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Parser)]
struct HostExecOpts {
    #[clap(allow_hyphen_values = true)]
    args: Vec<OsString>,
}

#[derive(Subcommand)]
enum Commands {
    InitEnvironment,
    /// Execute a command in the host context
    Hostexec(HostExecOpts),
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

#[tokio::main(flavor = "current_thread")]
#[instrument]
async fn main() -> Result<(), Report> {
    install_tracing();
    color_eyre::install()?;

    let cli = Cli::parse();

    match cli.command {
        Commands::InitEnvironment => {
            let e = envdetect::Environment::new()?;
            serde_json::to_writer(std::io::stdout(), &e)?;
            if e.container && e.privileged && e.pidhost {
                hostexec::prepare()?;
            }
        }
        Commands::Hostexec(opts) => {
            hostexec::run(opts.args)?;
        }
    }
    Ok(())
}
