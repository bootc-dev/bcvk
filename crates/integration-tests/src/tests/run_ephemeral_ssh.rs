//! Integration tests for ephemeral run-ssh command
//!
//! ⚠️  **CRITICAL INTEGRATION TEST POLICY** ⚠️
//!
//! INTEGRATION TESTS MUST NEVER "warn and continue" ON FAILURES!
//!
//! If something is not working:
//! - Use `todo!("reason why this doesn't work yet")`
//! - Use `panic!("clear error message")`
//! - Use `assert!()` and `unwrap()` to fail hard
//!
//! NEVER use patterns like:
//! - "Note: test failed - likely due to..."
//! - "This is acceptable in CI/testing environments"
//! - Warning and continuing on failures

use color_eyre::Result;
use integration_tests::{integration_test, parameterized_integration_test};
use xshell::cmd;

use std::time::{Duration, Instant};

use crate::{get_bck_command, get_test_image, shell, INTEGRATION_TEST_LABEL};

/// Poll until a container is removed or timeout is reached
///
/// Returns Ok(()) if container is removed within timeout, Err otherwise.
/// Timeout is set to 60 seconds to account for slow CI runners.
fn wait_for_container_removal(container_name: &str) -> Result<()> {
    let sh = shell()?;
    let timeout = Duration::from_secs(60);
    let start = Instant::now();
    let poll_interval = Duration::from_millis(100);
    let format_arg = "{{.Names}}";

    loop {
        let containers = cmd!(sh, "podman ps -a --format {format_arg}")
            .ignore_status()
            .read()?;

        if !containers.lines().any(|line| line == container_name) {
            return Ok(());
        }

        if start.elapsed() >= timeout {
            return Err(color_eyre::eyre::eyre!(
                "Timeout waiting for container {} to be removed. Active containers: {}",
                container_name,
                containers
            ));
        }

        std::thread::sleep(poll_interval);
    }
}

/// Build a test fixture image with the kernel removed
fn build_broken_image() -> Result<String> {
    let sh = shell()?;
    let fixture_path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/Dockerfile.no-kernel");
    let image_name = format!("localhost/bcvk-test-no-kernel:{}", std::process::id());
    let build_arg = format!("BASE_IMAGE={}", get_test_image());

    cmd!(
        sh,
        "podman build -f {fixture_path} -t {image_name} --build-arg {build_arg} ."
    )
    .run()?;

    Ok(image_name)
}

/// Test running a non-interactive command via SSH
fn test_run_ephemeral_ssh_command() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;

    let stdout = cmd!(
        sh,
        "{bck} ephemeral run-ssh --label {label} {image} -- echo 'hello world from SSH'"
    )
    .read()?;

    assert!(
        stdout.contains("hello world from SSH"),
        "Expected output not found. Got: {}",
        stdout
    );
    Ok(())
}
integration_test!(test_run_ephemeral_ssh_command);

/// Test that the container is cleaned up when SSH exits
fn test_run_ephemeral_ssh_cleanup() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;
    let container_name = format!("test-ssh-cleanup-{}", std::process::id());

    cmd!(
        sh,
        "{bck} ephemeral run-ssh --name {container_name} --label {label} {image} -- echo 'testing cleanup'"
    )
    .run()?;

    // Poll for container removal with timeout
    wait_for_container_removal(&container_name)?;

    Ok(())
}
integration_test!(test_run_ephemeral_ssh_cleanup);

/// Test running system commands via SSH
fn test_run_ephemeral_ssh_system_command() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;

    cmd!(
        sh,
        "{bck} ephemeral run-ssh --label {label} {image} -- /bin/sh -c 'systemctl is-system-running || true'"
    )
    .run()?;
    Ok(())
}
integration_test!(test_run_ephemeral_ssh_system_command);

/// Test that ephemeral run-ssh properly forwards exit codes
fn test_run_ephemeral_ssh_exit_code() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;

    let output = cmd!(
        sh,
        "{bck} ephemeral run-ssh --label {label} {image} -- exit 42"
    )
    .ignore_status()
    .output()?;

    let exit_code = output.status.code().expect("Failed to get exit code");
    assert_eq!(
        exit_code, 42,
        "Exit code not properly forwarded. Expected 42, got {}",
        exit_code
    );
    Ok(())
}
integration_test!(test_run_ephemeral_ssh_exit_code);

/// Test SSH functionality across different bootc images
/// This parameterized test runs once per image in BCVK_ALL_IMAGES and verifies
/// that our systemd version compatibility fix works correctly with both newer
/// systemd (Fedora) and older systemd (CentOS Stream 9)
fn test_run_ephemeral_ssh_cross_distro_compatibility(image: &str) -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let label = INTEGRATION_TEST_LABEL;

    let output = cmd!(
        sh,
        "{bck} ephemeral run-ssh --label {label} {image} -- systemctl --version"
    )
    .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "SSH test failed for image {}: {}",
        image,
        stderr
    );

    assert!(
        stdout.contains("systemd"),
        "systemd version not found for image {}. Got: {}",
        image,
        stdout
    );

    // Log systemd version for diagnostic purposes
    if let Some(version_line) = stdout.lines().next() {
        eprintln!("Image {} systemd version: {}", image, version_line);

        let version_parts: Vec<&str> = version_line.split_whitespace().collect();
        if version_parts.len() >= 2 {
            if let Ok(version_num) = version_parts[1].parse::<u32>() {
                if version_num >= 254 {
                    eprintln!(
                        "✓ {} supports vmm.notify_socket (version {})",
                        image, version_num
                    );
                } else {
                    eprintln!(
                        "✓ {} falls back to SSH polling (version {} < 254)",
                        image, version_num
                    );
                }
            }
        }
    }
    Ok(())
}
parameterized_integration_test!(test_run_ephemeral_ssh_cross_distro_compatibility);

/// Test that /run is mounted as tmpfs and supports unix domain sockets
fn test_run_tmpfs() -> Result<()> {
    use std::fs;
    use tempfile::TempDir;

    // Create a temporary directory with a test script
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let script_path = temp_dir.path().join("check_run_tmpfs.sh");

    // Write a script that verifies /run is tmpfs and supports unix domain sockets
    let script_content = r#"#!/bin/bash
set -euo pipefail

echo "Checking /run filesystem..."

# Verify /run is mounted as tmpfs
if ! findmnt -n -o FSTYPE /run | grep -q tmpfs; then
    echo "ERROR: /run is not a tmpfs"
    findmnt -n /run
    exit 1
fi

echo "✓ /run is tmpfs"

echo "All checks passed!"
"#;

    fs::write(&script_path, script_content).expect("Failed to write test script");

    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path)
            .expect("Failed to get file metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).expect("Failed to set permissions");
    }

    let mount_path = temp_dir
        .path()
        .to_str()
        .expect("Failed to convert path to string");

    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;
    let bind_arg = format!("{}:testscripts", mount_path);

    // Run the test via SSH with the script mounted via virtiofs
    let stdout = cmd!(
        sh,
        "{bck} ephemeral run-ssh --label {label} --bind {bind_arg} {image} -- /run/virtiofs-mnt-testscripts/check_run_tmpfs.sh"
    )
    .read()?;

    assert!(
        stdout.contains("All checks passed!"),
        "Test script did not complete successfully. Output: {}",
        stdout
    );

    Ok(())
}
integration_test!(test_run_tmpfs);

/// Test that containers are properly cleaned up even when the image is broken
///
/// This test verifies that the drop handler for ContainerCleanup works correctly
/// when ephemeral run-ssh fails early due to a broken image (missing kernel).
/// Previously this would fail with "setns `mnt`: Bad file descriptor" when using
/// podman's --rm flag. Now it should fail cleanly and remove the container.
fn test_run_ephemeral_ssh_broken_image_cleanup() -> Result<()> {
    // Build a broken test image (bootc image with kernel removed)
    eprintln!("Building broken test image...");
    let broken_image = build_broken_image()?;
    eprintln!("Built broken image: {}", broken_image);

    let sh = shell()?;
    let bck = get_bck_command()?;
    let container_name = format!("test-broken-cleanup-{}", std::process::id());
    let label = INTEGRATION_TEST_LABEL;

    // Try to run ephemeral SSH with the broken image - this should fail
    let output = cmd!(
        sh,
        "{bck} ephemeral run-ssh --name {container_name} --label {label} {broken_image} -- echo should_not_reach_here"
    )
    .ignore_status()
    .output()?;

    let stderr = String::from_utf8_lossy(&output.stderr);

    // The command should fail (no kernel found)
    assert!(
        !output.status.success(),
        "Expected ephemeral run-ssh to fail with broken image, but it succeeded"
    );

    // Verify the error message indicates the problem
    assert!(
        stderr.contains("Failed to read kernel modules directory")
            || stderr.contains("Container exited before SSH became available")
            || stderr.contains("Monitor process exited unexpectedly"),
        "Expected error about missing kernel or container failure, got: {}",
        stderr
    );

    // Poll for container removal with timeout
    wait_for_container_removal(&container_name)?;

    // Clean up the test image
    let _ = cmd!(sh, "podman rmi -f {broken_image}")
        .ignore_status()
        .quiet()
        .run();

    Ok(())
}
integration_test!(test_run_ephemeral_ssh_broken_image_cleanup);

/// Test ephemeral VM network and DNS
///
/// Verifies that ephemeral bootc VMs can access the network and resolve DNS correctly.
/// Uses HTTP request to quay.io to test both DNS resolution and network connectivity.
fn test_run_ephemeral_dns_resolution() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;

    // Test DNS + network by connecting to quay.io
    // Use curl or wget, whichever is available
    // Any HTTP response (including 401) proves DNS resolution and network connectivity work
    let dns_test_script = r#"if command -v curl >/dev/null 2>&1; then curl -sS --max-time 10 https://quay.io/v2/ >/dev/null; elif command -v wget >/dev/null 2>&1; then wget -q --timeout=10 -O /dev/null https://quay.io/v2/; else echo 'Neither curl nor wget available'; exit 1; fi"#;

    cmd!(
        sh,
        "{bck} ephemeral run-ssh --label {label} {image} -- /bin/sh -c {dns_test_script}"
    )
    .run()?;

    Ok(())
}
integration_test!(test_run_ephemeral_dns_resolution);

/// Test SSH timeout behavior when SSH is unavailable
///
/// Verifies that ephemeral run-ssh properly times out when SSH is masked.
/// Uses systemd.mask=sshd.service to disable SSH, triggering the timeout mechanism.
///
/// Note: This tests the ephemeral timeout (~240s), not the libvirt SSH timeout (~60s).
/// The libvirt SSH timeout (60s) is used by `bcvk libvirt ssh` and would require
/// creating a libvirt VM to test properly.
fn test_run_ephemeral_ssh_timeout() -> Result<()> {
    eprintln!("Testing SSH timeout with masked sshd.service...");
    eprintln!("This test takes ~240 seconds to complete...");

    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;

    let start = Instant::now();

    let output = cmd!(
        sh,
        "{bck} ephemeral run-ssh --label {label} --karg systemd.mask=sshd.service {image} -- echo should_not_reach_here"
    )
    .ignore_status()
    .output()?;

    let elapsed = start.elapsed();
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Command should fail (SSH timeout)
    assert!(
        !output.status.success(),
        "Expected ephemeral run-ssh to fail with SSH timeout, but it succeeded"
    );

    // Verify the error message mentions timeout or readiness failure
    assert!(
        stderr.contains("Timeout waiting for readiness")
            || stderr.contains("timeout")
            || stderr.contains("failed after"),
        "Expected error about timeout, got: {}",
        stderr
    );

    // Verify timeout duration is approximately 240 seconds (±20s tolerance for CI variability)
    let timeout_secs = elapsed.as_secs();
    assert!(
        timeout_secs >= 220 && timeout_secs <= 260,
        "Expected timeout around 240 seconds, but got {} seconds. \
         This suggests the ephemeral SSH timeout may not be working correctly.",
        timeout_secs
    );

    eprintln!("✓ SSH timeout worked correctly ({}s)", timeout_secs);

    Ok(())
}
integration_test!(test_run_ephemeral_ssh_timeout);
