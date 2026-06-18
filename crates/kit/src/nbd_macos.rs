//! NBD server management for macOS ephemeral VMs.
//!
//! macOS-specific: gvproxy expose API for TCP port forwarding.
//! Common logic (deploy, systemd-run, stop) lives in vm_helpers.rs.

use color_eyre::{eyre::bail, Result};
use indicatif::ProgressBar;
use std::time::Duration;
use tracing::info;

use crate::utils::wait_for_readiness;
use crate::vm_helpers;

/// NBD server binary (Linux ELF for host arch), embedded at compile time.
const NBD_SERVER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bcvk-nbd"));

/// Deploy the NBD server binary to the podman machine.
pub(crate) fn deploy_nbd_server(machine: &str) -> Result<()> {
    vm_helpers::deploy_nbd_server(machine, NBD_SERVER)
}

/// Start the NBD server via systemd-run and expose the port via gvproxy.
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
    let gw = vm_helpers::GVPROXY_GATEWAY;
    let vm_ip = vm_helpers::GVPROXY_VM_IP;
    let unexpose_cmd = format!(
        "curl -s -X POST http://{gw}:80/services/forwarder/unexpose \
         -H 'Content-Type: application/json' \
         -d '{{\"local\":\":{nbd_port}\",\"protocol\":\"tcp\"}}' >/dev/null 2>&1; true",
    );
    if let Err(e) = vm_helpers::machine_ssh(machine, &unexpose_cmd) {
        tracing::debug!("failed to unexpose port {}: {}", nbd_port, e);
    }

    let expose_cmd = format!(
        "curl -s -X POST http://{gw}:80/services/forwarder/expose \
         -H 'Content-Type: application/json' \
         -d '{{\"local\":\":{nbd_port}\",\"remote\":\"{vm_ip}:{nbd_port}\",\"protocol\":\"tcp\"}}'",
    );
    let expose_cmd_clone = expose_cmd.clone();
    let machine_clone = machine.to_string();
    wait_for_readiness(
        ProgressBar::hidden(),
        &format!("Exposing NBD port {}", nbd_port),
        move || match vm_helpers::machine_ssh_output(&machine_clone, &expose_cmd_clone) {
            Ok(output) if output.status.success() => Ok(true),
            _ => Ok(false),
        },
        Duration::from_secs(5),
        Duration::from_millis(500),
    )
    .map_err(|_| color_eyre::eyre::eyre!("gvproxy expose failed for port {}", nbd_port))?;

    info!("waiting for nbd server on port {}...", nbd_port);
    let machine_clone = machine.to_string();
    let unit_name_clone = unit_name.clone();
    wait_for_readiness(
        ProgressBar::hidden(),
        &format!("Waiting for NBD server on port {}", nbd_port),
        move || {
            if vm_helpers::is_nbd_unit_dead(&machine_clone, &unit_name_clone) {
                bail!(
                    "nbd server '{}' died before becoming ready on port {}",
                    unit_name_clone,
                    nbd_port
                );
            }
            if let Ok(mut stream) = std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], nbd_port)),
                Duration::from_millis(500),
            ) {
                use std::io::Read;
                stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
                let mut buf = [0u8; 8];
                if stream.read_exact(&mut buf).is_ok() && &buf == b"NBDMAGIC" {
                    return Ok(true);
                }
            }
            Ok(false)
        },
        Duration::from_secs(30),
        Duration::from_millis(500),
    )?;

    Ok(unit_name)
}

/// NBD port allocation range start (inclusive).
const NBD_PORT_RANGE_START: u16 = 10800;
/// NBD port allocation range end (exclusive).
const NBD_PORT_RANGE_END: u16 = 10900;

/// Find an available TCP port for NBD in range 10800-10900.
pub fn find_available_nbd_port() -> u16 {
    vm_helpers::find_available_port_in_range(NBD_PORT_RANGE_START, NBD_PORT_RANGE_END)
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
                    "curl -sf -X POST http://{}:80/services/forwarder/unexpose \
                     -H 'Content-Type: application/json' \
                     -d '{{\"local\":\":{}\",\"protocol\":\"tcp\"}}'",
                    vm_helpers::GVPROXY_GATEWAY,
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
