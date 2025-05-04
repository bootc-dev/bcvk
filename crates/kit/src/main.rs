use std::ffi::OsString;

use anyhow::Result;
use clap::{Parser, Subcommand};

pub(crate) mod containerenv;
mod hostexec;
mod vm;
mod envdetect;

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
    DetectEnv,
    /// Execute a command in the host context
    Hostexec(HostExecOpts),
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::DetectEnv => {
            let e = envdetect::Environment::new()?;
            serde_json::to_writer(std::io::stdout(), &e)?;
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
