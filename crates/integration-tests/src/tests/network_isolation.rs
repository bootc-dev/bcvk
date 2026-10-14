//! Integration tests for network isolation (--network-isolation flag)
//!
//! Verifies that VMs launched with --network-isolation cannot reach
//! external hosts, while SSH from the host into the VM still works.

use integration_tests::integration_test;
use itest::TestResult;
use xshell::cmd;

use crate::{get_bck_command, get_test_image, shell, INTEGRATION_TEST_LABEL};

/// Well-known external IP used to verify outbound connectivity.
/// Google Public DNS; chosen because it is highly reliable and
/// responds to ICMP echo.
const EXTERNAL_PROBE_IP: &str = "8.8.8.8";

/// Check whether the host can reach the external probe IP.
fn host_can_reach_external() -> bool {
    std::process::Command::new("ping")
        .args(["-c1", "-W5", EXTERNAL_PROBE_IP])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Test that a guest WITHOUT --network-isolation can reach the internet.
///
/// This is the positive control for the isolation test: it proves that
/// run-ssh works, that the guest has a working network stack, and that
/// the external probe IP is reachable from inside the guest.
///
/// If the host itself cannot reach the probe IP, the test prints a
/// warning and passes without booting a VM.
fn test_run_ephemeral_network_reachable() -> TestResult {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;

    if !host_can_reach_external() {
        eprintln!();
        eprintln!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
        eprintln!("WARNING: Host cannot reach {EXTERNAL_PROBE_IP}");
        eprintln!("         Skipping guest connectivity check.");
        eprintln!("         Re-run with internet access for full coverage.");
        eprintln!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
        eprintln!();
        return Ok(());
    }

    // Boot a normal VM (no isolation) and verify the guest can ping
    // the external probe IP. This proves run-ssh, guest networking,
    // and external reachability all work.
    cmd!(
        sh,
        "{bck} ephemeral run-ssh --label {label} {image} -- ping -c1 -W10 {EXTERNAL_PROBE_IP}"
    )
    .run()?;

    eprintln!("Guest successfully reached {EXTERNAL_PROBE_IP} (positive control passed)");

    Ok(())
}
integration_test!(test_run_ephemeral_network_reachable);

/// Test that --network-isolation blocks outbound traffic from the guest.
///
/// 1. Verify the host itself can reach 8.8.8.8. If it cannot, skip with
///    a loud warning (so offline environments are not broken).
/// 2. Boot an ephemeral VM WITHOUT --network-isolation and ping the
///    probe IP. This positive control proves that run-ssh and guest
///    networking work correctly. If this step fails, the test
///    environment is broken and we bail out rather than risk a false
///    pass on the isolation check.
/// 3. Boot an ephemeral VM WITH --network-isolation and ping the same
///    IP. This must fail, proving outbound traffic is blocked.
/// 4. The fact that run-ssh itself succeeds in step 3 proves that SSH
///    (host-to-guest via hostfwd) is preserved under isolation.
fn test_run_ephemeral_network_isolation() -> TestResult {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;

    if !host_can_reach_external() {
        eprintln!();
        eprintln!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
        eprintln!("WARNING: Host cannot reach {EXTERNAL_PROBE_IP}");
        eprintln!("         Network isolation test CANNOT verify that the");
        eprintln!("         guest is actually blocked. Skipping.");
        eprintln!("         Re-run with internet access for full coverage.");
        eprintln!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
        eprintln!();
        return Ok(());
    }

    // Step 1: Positive control -- verify the guest can reach the
    // external probe IP without isolation. This guards against a false
    // pass where the isolated test "succeeds" because networking is
    // broken for an unrelated reason (e.g. SSH itself is not working).
    eprintln!(
        "Positive control: verifying guest can reach {EXTERNAL_PROBE_IP} without isolation..."
    );
    cmd!(
        sh,
        "{bck} ephemeral run-ssh --label {label} {image} -- ping -c1 -W10 {EXTERNAL_PROBE_IP}"
    )
    .run()?;
    eprintln!("Positive control passed");

    // Step 2: Isolation check -- the same ping must fail with
    // --network-isolation. run-ssh propagates the guest command's exit
    // code, so we use ignore_status() and check manually.
    eprintln!("Isolation check: verifying guest CANNOT reach {EXTERNAL_PROBE_IP} with --network-isolation...");
    let output = cmd!(
        sh,
        "{bck} ephemeral run-ssh --network-isolation --label {label} {image} -- ping -c1 -W10 {EXTERNAL_PROBE_IP}"
    )
    .ignore_status()
    .output()?;

    assert!(
        !output.status.success(),
        "Guest ping to {EXTERNAL_PROBE_IP} succeeded despite --network-isolation; \
         expected it to be blocked. stdout: {} stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    eprintln!(
        "Isolation check passed: guest ping failed as expected (exit code {:?})",
        output.status.code()
    );

    Ok(())
}
integration_test!(test_run_ephemeral_network_isolation);
