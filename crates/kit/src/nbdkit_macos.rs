//! nbdkit EROFS plugin management for macOS ephemeral VMs.

use color_eyre::{
    eyre::{bail, Context},
    Result,
};
use std::process::{Command, Stdio};
use std::time::Duration;
use tracing::info;

use crate::run_ephemeral_macos::detect_machine_name;

/// Path to the nbdkit EROFS plugin shared library inside podman machine.
const NBDKIT_EROFS_PLUGIN_PATH: &str = "/var/tmp/bcvk/libnbdkit_erofs_plugin.so";

/// Get the merged overlay path from podman image mount.
pub(crate) fn get_merged_path(machine: &str, rootful: bool, image: &str) -> Result<String> {
    let output = if rootful {
        Command::new("podman")
            .args([
                "machine", "ssh", machine, "--", "podman", "image", "mount", image,
            ])
            .output()
            .context("podman image mount")?
    } else {
        Command::new("podman")
            .args([
                "machine", "ssh", machine, "--", "podman", "unshare", "podman", "image", "mount",
                image,
            ])
            .output()
            .context("podman image mount")?
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("podman image mount failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Start nbdkit with the erofs plugin for dynamic EROFS + ESP + GPT generation.
pub(crate) fn start_nbdkit_erofs_plugin(
    machine: &str,
    merged_path: &str,
    cmdline: &str,
    ssh_pubkey: &str,
    nbd_port: u16,
    vm_name: &str,
) -> Result<String> {
    let container_name = format!("bcvk-nbd-{}", vm_name);

    let _ = Command::new("podman")
        .args([
            "machine",
            "ssh",
            machine,
            "--",
            "podman",
            "rm",
            "-f",
            &container_name,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    fn shell_escape(s: &str) -> String {
        format!("'{}'", s.replace('\'', "'\\''"))
    }

    let cmdline_esc = shell_escape(&format!("cmdline={}", cmdline));
    let dir_esc = shell_escape(&format!("dir={}", merged_path));

    let mut ssh_param = String::new();
    if !ssh_pubkey.is_empty() {
        ssh_param = format!(" {}", shell_escape(&format!("ssh_pubkey={}", ssh_pubkey)));
    }

    let podman_cmd = format!(
        "podman run -d --name {name} --security-opt label=disable \
         -p {port}:10809 \
         -v {merged}:{merged}:ro \
         -v {plugin}:/plugin.so:ro \
         -v /usr/bin/nbdkit:/usr/bin/nbdkit:ro \
         -v /usr/lib64/nbdkit:/usr/lib64/nbdkit:ro \
         quay.io/fedora/fedora:latest \
         nbdkit -f -p 10809 -r /plugin.so \
         {dir} {cmdline}{ssh}",
        name = container_name,
        port = nbd_port,
        merged = merged_path,
        plugin = NBDKIT_EROFS_PLUGIN_PATH,
        dir = dir_esc,
        cmdline = cmdline_esc,
        ssh = ssh_param,
    );

    let output = Command::new("podman")
        .args(["machine", "ssh", machine, "--", &podman_cmd])
        .output()
        .context("failed to start nbdkit erofs plugin")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("failed to start nbdkit erofs plugin: {}", stderr.trim());
    }

    info!("waiting for nbdkit on port {}...", nbd_port);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(mut stream) = std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], nbd_port)),
            Duration::from_millis(500),
        ) {
            use std::io::Read;
            stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
            let mut buf = [0u8; 8];
            if stream.read_exact(&mut buf).is_ok() && &buf == b"NBDMAGIC" {
                break;
            }
        }
        if std::time::Instant::now() > deadline {
            let _ = Command::new("podman")
                .args([
                    "machine",
                    "ssh",
                    machine,
                    "--",
                    "podman",
                    "rm",
                    "-f",
                    &container_name,
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            bail!(
                "nbdkit erofs plugin did not become ready on port {}",
                nbd_port
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    Ok(container_name)
}

/// Find an available TCP port for NBD in range 10800-10900.
pub fn find_available_nbd_port() -> u16 {
    use rand::Rng;
    let mut rng = rand::rng();
    const PORT_RANGE_START: u16 = 10800;
    const PORT_RANGE_END: u16 = 10900;
    for _ in 0..100 {
        let port = rng.random_range(PORT_RANGE_START..PORT_RANGE_END);
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    for port in PORT_RANGE_START..PORT_RANGE_END {
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    PORT_RANGE_START
}

/// Stop and remove an nbdkit container (best-effort).
pub fn stop_nbdkit_container(container_name: &str) {
    if let Ok(machine) = detect_machine_name() {
        let _ = Command::new("podman")
            .args([
                "machine",
                "ssh",
                &machine,
                "--",
                "podman",
                "rm",
                "-f",
                container_name,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
