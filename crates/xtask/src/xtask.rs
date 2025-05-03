//! See https://github.com/matklad/cargo-xtask
//! This is kind of like "Justfile but in Rust".

use std::process::Command;

use anyhow::{Context, Result};
use xshell::{cmd, Shell};

fn main() {
    if let Err(e) = try_main() {
        eprintln!("error: {e:?}");
        std::process::exit(1);
    }
}

#[allow(clippy::type_complexity)]
const TASKS: &[(&str, fn(&Shell) -> Result<()>)] = &[("build", build)];

fn try_main() -> Result<()> {
    // Ensure our working directory is the toplevel
    {
        let toplevel_path = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .context("Invoking git rev-parse")?;
        if !toplevel_path.status.success() {
            anyhow::bail!("Failed to invoke git rev-parse");
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
