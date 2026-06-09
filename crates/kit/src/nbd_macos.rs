//! NBD server management for macOS ephemeral VMs.
//!
//! macOS-specific: gvproxy expose API for TCP port forwarding.
//! Common logic (deploy, systemd-run, stop) lives in vm_helpers.rs.

use color_eyre::{eyre::bail, Result};
use std::time::Duration;
use tracing::info;

use crate::vm_helpers;

/// NBD server binary (aarch64 ELF), embedded at compile time.
const NBD_SERVER: &[u8] = include_bytes!("../bcvk-nbd-aarch64");

/// Deploy the NBD server binary to the podman machine.
pub(crate) fn deploy_nbd_server(machine: &str) -> Result<()> {
    vm_helpers::deploy_nbd_server(machine, NBD_SERVER)
}

/// Start the NBD server via systemd-run and expose the port via gvproxy.
#[allow(dead_code)]
pub(crate) fn start_nbd_server(
    machine: &str,
    merged_path: &str,
    cmdline: &str,
    ssh_pubkey: &str,
    nbd_port: u16,
    vm_name: &str,
) -> Result<String> {
    let unit_name = format!("bcvk-nbd-{}", vm_name);

    vm_helpers::start_nbd_unit(
        machine,
        &unit_name,
        merged_path,
        cmdline,
        ssh_pubkey,
        &format!("--port {}", nbd_port),
    )?;

    // macOS-specific: unexpose stale entry then expose via gvproxy's in-VM API
    let unexpose_cmd = format!(
        "curl -s -X POST http://192.168.127.1:80/services/forwarder/unexpose \
         -H 'Content-Type: application/json' \
         -d '{{\"local\":\":{nbd_port}\",\"protocol\":\"tcp\"}}' >/dev/null 2>&1; true",
        nbd_port = nbd_port,
    );
    if let Err(e) = vm_helpers::machine_ssh(machine, &unexpose_cmd) {
        tracing::debug!("failed to unexpose port {}: {}", nbd_port, e);
    }

    let expose_cmd = format!(
        "curl -s -X POST http://192.168.127.1:80/services/forwarder/expose \
         -H 'Content-Type: application/json' \
         -d '{{\"local\":\":{nbd_port}\",\"remote\":\"192.168.127.2:{nbd_port}\",\"protocol\":\"tcp\"}}'",
        nbd_port = nbd_port,
    );
    let mut exposed = false;
    for i in 0..5 {
        if let Ok(output) = vm_helpers::machine_ssh_output(machine, &expose_cmd) {
            if output.status.success() {
                exposed = true;
                break;
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            tracing::debug!(
                "gvproxy expose attempt {}: {}{}",
                i + 1,
                stdout.trim(),
                stderr.trim()
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if !exposed {
        bail!("gvproxy expose failed for port {}", nbd_port);
    }

    info!("waiting for nbd server on port {}...", nbd_port);
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
        if vm_helpers::is_nbd_unit_dead(machine, &unit_name) {
            bail!(
                "nbd server '{}' died before becoming ready on port {}",
                unit_name,
                nbd_port
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    Ok(unit_name)
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

/// Stop an NBD server and unexpose its gvproxy port (best-effort).
pub fn stop_nbd_server(unit_name: &str, nbd_port: Option<u16>) {
    if let Ok(machine) = vm_helpers::detect_machine_name() {
        vm_helpers::stop_nbd_unit(&machine, unit_name);
        // macOS-specific: unexpose gvproxy port
        if let Some(port) = nbd_port {
            if let Err(e) = vm_helpers::machine_ssh(
                &machine,
                &format!(
                    "curl -sf -X POST http://192.168.127.1:80/services/forwarder/unexpose \
                     -H 'Content-Type: application/json' \
                     -d '{{\"local\":\":{}\",\"protocol\":\"tcp\"}}'",
                    port
                ),
            ) {
                tracing::debug!("failed to unexpose port {}: {}", port, e);
            }
        }
    }
}

/// Re-export for run_ephemeral_macos.rs
pub use vm_helpers::get_merged_path;
