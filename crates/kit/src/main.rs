use std::{ffi::OsString, os::unix::process::CommandExt, process::Command};

use anyhow::Result;
use clap::{Parser, Subcommand};

mod containerenv;
mod hostexec;
mod runscript;
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
    /// Run an interactive shell
    Shell,
    OutputEntrypoint,
    /// Execute a command in the host context
    Hostexec(HostExecOpts),
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Shell => {
            indoc::printdoc! { r#"
            bootc-kit, an interactive container dev environment for https://github.com/bootc-dev/bootc
            This command shell is an instance of https://www.nushell.sh/ extended with custom commands.
            Type `help` for more information.
            "# };
            Err(Command::new("nu").exec())?
        }
        Commands::Hostexec(opts) => {
            hostexec::run(opts.args)?;
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
