use std::ffi::OsString;

use color_eyre::Result;
use clap::{Parser, Subcommand};

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

async fn run() -> Result<()> {
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

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
