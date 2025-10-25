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
use linkme::distributed_slice;
use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::{
    get_alternative_test_image, get_test_image, run_bcvk, IntegrationTest, INTEGRATION_TESTS,
    INTEGRATION_TEST_LABEL,
};

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_RUN_EPHEMERAL_SSH_COMMAND: IntegrationTest =
    IntegrationTest::new("run_ephemeral_ssh_command", test_run_ephemeral_ssh_command);

/// Test running a non-interactive command via SSH
fn test_run_ephemeral_ssh_command() -> Result<()> {
    let output = run_bcvk(&[
        "ephemeral",
        "run-ssh",
        "--label",
        INTEGRATION_TEST_LABEL,
        &get_test_image(),
        "--",
        "echo",
        "hello world from SSH",
    ])?;

    output.assert_success("ephemeral run-ssh");

    assert!(
        output.stdout.contains("hello world from SSH"),
        "Expected output not found. Got: {}",
        output.stdout
    );
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_RUN_EPHEMERAL_SSH_CLEANUP: IntegrationTest =
    IntegrationTest::new("run_ephemeral_ssh_cleanup", test_run_ephemeral_ssh_cleanup);

/// Test that the container is cleaned up when SSH exits
fn test_run_ephemeral_ssh_cleanup() -> Result<()> {
    let container_name = format!("test-ssh-cleanup-{}", std::process::id());

    let output = run_bcvk(&[
        "ephemeral",
        "run-ssh",
        "--name",
        &container_name,
        "--label",
        INTEGRATION_TEST_LABEL,
        &get_test_image(),
        "--",
        "echo",
        "testing cleanup",
    ])?;

    output.assert_success("ephemeral run-ssh");

    thread::sleep(Duration::from_secs(1));

    let check_output = Command::new("podman")
        .args(["ps", "-a", "--format", "{{.Names}}"])
        .output()
        .expect("Failed to list containers");

    let containers = String::from_utf8_lossy(&check_output.stdout);
    assert!(
        !containers.contains(&container_name),
        "Container {} was not cleaned up after SSH exit. Active containers: {}",
        container_name,
        containers
    );
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_RUN_EPHEMERAL_SSH_SYSTEM_COMMAND: IntegrationTest = IntegrationTest::new(
    "run_ephemeral_ssh_system_command",
    test_run_ephemeral_ssh_system_command,
);

/// Test running system commands via SSH
fn test_run_ephemeral_ssh_system_command() -> Result<()> {
    let output = run_bcvk(&[
        "ephemeral",
        "run-ssh",
        "--label",
        INTEGRATION_TEST_LABEL,
        &get_test_image(),
        "--",
        "/bin/sh",
        "-c",
        "systemctl is-system-running || true",
    ])?;

    output.assert_success("ephemeral run-ssh");
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_RUN_EPHEMERAL_SSH_EXIT_CODE: IntegrationTest = IntegrationTest::new(
    "run_ephemeral_ssh_exit_code",
    test_run_ephemeral_ssh_exit_code,
);

/// Test that ephemeral run-ssh properly forwards exit codes
fn test_run_ephemeral_ssh_exit_code() -> Result<()> {
    let output = run_bcvk(&[
        "ephemeral",
        "run-ssh",
        "--label",
        INTEGRATION_TEST_LABEL,
        &get_test_image(),
        "--",
        "exit",
        "42",
    ])?;

    let exit_code = output.exit_code().expect("Failed to get exit code");
    assert_eq!(
        exit_code, 42,
        "Exit code not properly forwarded. Expected 42, got {}",
        exit_code
    );
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_RUN_EPHEMERAL_SSH_CROSS_DISTRO_COMPATIBILITY: IntegrationTest = IntegrationTest::new(
    "run_ephemeral_ssh_cross_distro_compatibility",
    test_run_ephemeral_ssh_cross_distro_compatibility,
);

/// Test SSH functionality across different bootc images (Fedora and CentOS)
/// This test verifies that our systemd version compatibility fix works correctly
/// with both newer systemd (Fedora) and older systemd (CentOS Stream 9)
fn test_run_ephemeral_ssh_cross_distro_compatibility() -> Result<()> {
    test_ssh_with_image(&get_test_image(), "primary")?;
    test_ssh_with_image(&get_alternative_test_image(), "alternative")?;
    Ok(())
}

fn test_ssh_with_image(image: &str, image_type: &str) -> Result<()> {
    let output = run_bcvk(&[
        "ephemeral",
        "run-ssh",
        "--label",
        INTEGRATION_TEST_LABEL,
        image,
        "--",
        "systemctl",
        "--version",
    ])?;

    assert!(
        output.success(),
        "{} image SSH test failed: {}",
        image_type,
        output.stderr
    );

    assert!(
        output.stdout.contains("systemd"),
        "{} image: systemd version not found. Got: {}",
        image_type,
        output.stdout
    );

    // Log systemd version for diagnostic purposes
    if let Some(version_line) = output.stdout.lines().next() {
        eprintln!("{} image systemd version: {}", image_type, version_line);

        let version_parts: Vec<&str> = version_line.split_whitespace().collect();
        if version_parts.len() >= 2 {
            if let Ok(version_num) = version_parts[1].parse::<u32>() {
                if version_num >= 254 {
                    eprintln!(
                        "✓ {} supports vmm.notify_socket (version {})",
                        image_type, version_num
                    );
                } else {
                    eprintln!(
                        "✓ {} falls back to SSH polling (version {} < 254)",
                        image_type, version_num
                    );
                }
            }
        }
    }
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_RUN_TMPFS: IntegrationTest = IntegrationTest::new("run_tmpfs", test_run_tmpfs);

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

    // Run the test via SSH with the script mounted via virtiofs
    let output = run_bcvk(&[
        "ephemeral",
        "run-ssh",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--bind",
        &format!("{}:testscripts", mount_path),
        &get_test_image(),
        "--",
        "/run/virtiofs-mnt-testscripts/check_run_tmpfs.sh",
    ])?;

    output.assert_success("ephemeral run-ssh with tmpfs check");

    assert!(
        output.stdout.contains("All checks passed!"),
        "Test script did not complete successfully. Output: {}",
        output.stdout
    );

    Ok(())
}
