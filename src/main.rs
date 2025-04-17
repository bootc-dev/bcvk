use anyhow::Result;
use clap::{Parser, Subcommand};

mod hostexec;
mod runscript;
mod vm;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Parser)]
struct RunOpts {
    /// Name of the container image to run
    image: String,
}

#[derive(Subcommand)]
enum Commands {
    OutputEntrypoint,
    /// Run the bootc container image within an ephemeral VM.
    Run(RunOpts),
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run(opts) => {
            hostexec::run(["podman", "inspect", &opts.image])?;
        }
        Commands::OutputEntrypoint => {
            runscript::print(&mut std::io::stdout().lock())?;
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
