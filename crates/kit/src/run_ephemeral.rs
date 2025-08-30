use std::io::Write;
use std::process::Command;

use clap::Parser;
use color_eyre::eyre::{eyre, Context};
use color_eyre::Result;
use itertools::Itertools;
use tracing::{info, instrument};

#[derive(Parser, Debug)]
pub struct RunEphemeralOpts {
    /// Container image to run as VM
    pub image: String,

    /// Memory in MiB
    #[clap(long, default_value_t = 2048)]
    pub memory: u32,

    /// Number of vCPUs
    #[clap(long, default_value_t = 2)]
    pub vcpus: u32,

    /// Additional kernel command line arguments
    #[clap(long = "karg")]
    pub kernel_args: Vec<String>,

    #[clap(long, default_value = "none")]
    pub net: String,

    /// Disable console output to terminal
    #[clap(long)]
    pub no_console: bool,

    /// Enable debug mode (drop into shell instead of running QEMU)
    #[clap(long)]
    pub debug: bool,
}

#[instrument]
fn run_qemu_in_container(opts: &RunEphemeralOpts) -> Result<std::process::ExitStatus> {
    info!("Running QEMU inside hybrid container for {}", opts.image);

    // TODO: Instead of generating shell scripts, we should:
    // 1. Build this binary as a static musl target (x86_64-unknown-linux-musl)
    // 2. Bind mount the static binary into the target container
    // 3. Have the binary handle hybrid rootfs setup and QEMU execution in Rust
    // 4. This avoids "shell script in Rust" anti-pattern and is more reliable
    // 5. Static linking ensures no dependency issues in different container environments
    //
    // For now, using shell script approach as proof-of-concept.

    // Load the script template from external file and replace placeholders
    // This is cleaner than having shell script embedded in Rust format strings
    let script_template = include_str!("../scripts/run_qemu.sh");

    let console_args = if !opts.no_console {
        " console=ttyS0"
    } else {
        ""
    };

    let extra_args = opts
        .kernel_args
        .iter()
        .map(|s| s.as_str())
        .chain((!opts.no_console).then_some("console=ttyS0"))
        .join(" ");

    let console_qemu_args = if !opts.no_console {
        "QEMU_ARGS+=(-serial stdio -display none)"
    } else {
        "# Console disabled"
    };

    let setup_script = script_template
        .replace("{{MEMORY}}", &opts.memory.to_string())
        .replace("{{VCPUS}}", &opts.vcpus.to_string())
        .replace("{{CONSOLE_ARGS}}", console_args)
        .replace("{{EXTRA_ARGS}}", &extra_args)
        .replace("{{CONSOLE_QEMU_ARGS}}", console_qemu_args);

    let mut tmp_script = tempfile::NamedTempFile::new()?;
    tmp_script.write_all(setup_script.as_bytes())?;
    tmp_script.flush()?;

    // Make it executable
    #[cfg(unix)]
    {
        use std::{fs::Permissions, os::unix::fs::PermissionsExt};
        let perms = Permissions::from_mode(0o755);
        tmp_script.as_file().set_permissions(perms)?;
    }
    let tmp_script = tmp_script.into_temp_path();
    let tmp_script = tmp_script.to_str().unwrap();

    info!("Starting QEMU in hybrid container");

    // Run the container with the setup script
    let mut cmd = Command::new("podman");
    cmd.arg("run");
    cmd.arg(format!("--net={}", opts.net.as_str()));
    cmd.args([
        "--rm",
        "-it", // Interactive for console passthrough
        // Needed to create nested containers (mountns, etc). Note when running
        // with userns (podman unpriv default) this is totally safe. TODO:
        // Default to enabling userns when running rootful.
        "--cap-add=all",
        // We mount the host /usr (though just *read-only*) but to do that we need to
        // disable default SELinux confinement
        "--security-opt=label=disable",
        // Also needed for nested containers
        "--security-opt=seccomp=unconfined",
        "--security-opt=unmask=/proc/*",
        // This is a general hardening thing to do when running privileged
        "-v",
        "/sys:/sys:ro",
        "--device=/dev/kvm",
        "-v",
        "/usr:/run/hostusr:ro", // Bind mount host /usr as read-only
        "-v",
        &format!("{tmp_script}:/run/entrypoint"),
        // And bind mount in the pristine image (without any mounts on top)
        // that we'll use as a mount source for virtiofs.
        &format!(
            "--mount=type=image,source={},target=/run/source-image",
            opts.image.as_str()
        ),
    ]);

    // Set debug mode environment variable if requested
    if opts.debug {
        cmd.args(["-e", "DEBUG_MODE=true"]);
        info!("Debug mode enabled - will drop into shell instead of running QEMU");
    }

    let status = cmd
        .args([&opts.image, "/run/entrypoint"])
        .status()
        .context("Failed to run QEMU in container")?;

    Ok(status)
}

#[instrument]
pub fn run(opts: RunEphemeralOpts) -> Result<()> {
    // Run QEMU inside the container with the hybrid rootfs approach
    let status = run_qemu_in_container(&opts)?;
    if !status.success() {
        return Err(eyre!("QEMU exited with non-zero status"));
    }

    info!("VM terminated successfully");
    Ok(())
}
