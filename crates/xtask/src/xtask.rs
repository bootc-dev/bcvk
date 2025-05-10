//! See https://github.com/matklad/cargo-xtask
//! This is kind of like "Justfile but in Rust".

use std::process::Command;

use color_eyre::eyre::{eyre, Context, Report};
use color_eyre::Result;
use tracing::instrument;
use xshell::{cmd, Shell};

#[allow(clippy::type_complexity)]
const TASKS: &[(&str, fn(&Shell) -> Result<()>)] = &[("build", build)];

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
    // Ensure our working directory is the toplevel
    {
        let toplevel_path = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .context("Invoking git rev-parse")?;
        if !toplevel_path.status.success() {
            return Err(eyre!("Failed to invoke git rev-parse"));
        }
        let path = String::from_utf8(toplevel_path.stdout)?;
        std::env::set_current_dir(path.trim()).context("Changing to toplevel")?;
    }

    let task = std::env::args().nth(1);

    let sh = xshell::Shell::new()?;
    if let Some(cmd) = task.as_deref() {
        let f = TASKS
            .iter()
            .find_map(|(k, f)| (*k == cmd).then_some(*f))
            .unwrap_or(print_help);
        f(&sh)?;
    } else {
        print_help(&sh)?;
    }
    Ok(())
}

#[instrument(skip(sh), name = "build")]
fn build(sh: &Shell) -> Result<()> {
    cmd!(sh, "cargo build -p bootc-kit --release").run()?;
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")] {
            cmd!(sh, "cargo build -p agent-macos --release").run()?;
        } else if #[cfg(target_os = "linux")] {
            // Nothing, we can just use systemd-run
        } else {
            compile_error!("Unsupported OS")
        }
    }
    Ok(())
}

fn print_help(_sh: &Shell) -> Result<()> {
    println!("Tasks:");
    for (name, _) in TASKS {
        println!("  - {name}");
    }
    Ok(())
}
