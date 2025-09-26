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

use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::{get_alternative_test_image, get_bck_command, get_test_image, INTEGRATION_TEST_LABEL};

/// Test running a non-interactive command via SSH
pub fn test_run_ephemeral_ssh_command() {
    let bck = get_bck_command().unwrap();

    eprintln!("Testing ephemeral run-ssh with command execution...");

    // Run ephemeral SSH with a simple echo command
    let output = Command::new("timeout")
        .args([
            "60s",
            &bck,
            "ephemeral",
            "run-ssh",
            "--label",
            INTEGRATION_TEST_LABEL,
            &get_test_image(),
            "--",
            "echo",
            "hello world from SSH",
        ])
        .output()
        .expect("Failed to run bcvk ephemeral run-ssh");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    eprintln!("stdout: {}", stdout);
    eprintln!("stderr: {}", stderr);

    // Check that the command completed successfully
    assert!(
        output.status.success(),
        "ephemeral run-ssh failed: {}",
        stderr
    );

    // Check that we got the expected output
    assert!(
        stdout.contains("hello world from SSH"),
        "Expected output not found. Got: {}",
        stdout
    );

    eprintln!("Successfully executed command via SSH and received output");
}

/// Test that the container is cleaned up when SSH exits
pub fn test_run_ephemeral_ssh_cleanup() {
    let bck = get_bck_command().unwrap();

    eprintln!("Testing ephemeral run-ssh cleanup behavior...");

    // Generate a unique container name for this test
    let container_name = format!("test-ssh-cleanup-{}", std::process::id());

    // Run ephemeral SSH with a simple command
    let output = Command::new("timeout")
        .args([
            "60s",
            &bck,
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
        ])
        .output()
        .expect("Failed to run bcvk ephemeral run-ssh");

    assert!(
        output.status.success(),
        "ephemeral run-ssh failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Give a moment for cleanup to complete
    thread::sleep(Duration::from_secs(1));

    // Check that the container no longer exists
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

    eprintln!("Container was successfully cleaned up after SSH exit");
}

/// Test running system commands via SSH
pub fn test_run_ephemeral_ssh_system_command() {
    let bck = get_bck_command().unwrap();

    eprintln!("Testing ephemeral run-ssh with system command...");

    // Run ephemeral SSH with systemctl command - using /bin/sh -c for shell operators
    let output = Command::new("timeout")
        .args([
            "60s",
            &bck,
            "ephemeral",
            "run-ssh",
            "--label",
            INTEGRATION_TEST_LABEL,
            &get_test_image(),
            "--",
            "/bin/sh",
            "-c",
            "systemctl is-system-running || true", // Shell command with operator
        ])
        .output()
        .expect("Failed to run bcvk ephemeral run-ssh");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    eprintln!("stdout: {}", stdout);
    eprintln!("stderr: {}", stderr);

    // The command should complete (even if system is degraded)
    assert!(
        output.status.success(),
        "ephemeral run-ssh failed: {}",
        stderr
    );

    eprintln!("Successfully executed system command via SSH");
}

/// Test that ephemeral run-ssh properly forwards exit codes
pub fn test_run_ephemeral_ssh_exit_code() {
    let bck = get_bck_command().unwrap();

    eprintln!("Testing ephemeral run-ssh exit code forwarding...");

    // Run a command that exits with non-zero code
    let output = Command::new("timeout")
        .args([
            "60s",
            &bck,
            "ephemeral",
            "run-ssh",
            "--label",
            INTEGRATION_TEST_LABEL,
            &get_test_image(),
            "--",
            "exit",
            "42",
        ])
        .output()
        .expect("Failed to run bcvk ephemeral run-ssh");

    // Check that the exit code was properly forwarded
    let exit_code = output.status.code().expect("Failed to get exit code");
    assert_eq!(
        exit_code, 42,
        "Exit code not properly forwarded. Expected 42, got {}",
        exit_code
    );

    eprintln!("Exit code was properly forwarded");
}

/// Test SSH functionality across different bootc images (Fedora and CentOS)
/// This test verifies that our systemd version compatibility fix works correctly
/// with both newer systemd (Fedora) and older systemd (CentOS Stream 9)
pub fn test_run_ephemeral_ssh_cross_distro_compatibility() {
    let bck = get_bck_command().unwrap();

    // Test with primary image (usually Fedora)
    test_ssh_with_image(&bck, &get_test_image(), "primary");

    // Test with alternative image (usually CentOS Stream 9)
    test_ssh_with_image(&bck, &get_alternative_test_image(), "alternative");
}

fn test_ssh_with_image(bck: &str, image: &str, image_type: &str) {
    eprintln!(
        "Testing SSH functionality with {} image: {}",
        image_type, image
    );

    // Test basic SSH connectivity and systemd status
    let output = Command::new("timeout")
        .args([
            "90s", // Longer timeout for potentially slower images
            bck,
            "ephemeral",
            "run-ssh",
            "--label",
            INTEGRATION_TEST_LABEL,
            image,
            "--",
            "systemctl",
            "--version",
        ])
        .output()
        .expect("Failed to run bcvk ephemeral run-ssh");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    eprintln!("=== {} image output ===", image_type);
    eprintln!("stdout: {}", stdout);
    eprintln!("stderr: {}", stderr);
    eprintln!("exit code: {:?}", output.status.code());

    // Check that the SSH connection was successful
    assert!(
        output.status.success(),
        "{} image SSH test failed: {}",
        image_type,
        stderr
    );

    // Verify we got systemd version output
    assert!(
        stdout.contains("systemd"),
        "{} image: systemd version not found. Got: {}",
        image_type,
        stdout
    );

    // Extract and log systemd version for compatibility verification
    if let Some(version_line) = stdout.lines().next() {
        eprintln!("{} image systemd version: {}", image_type, version_line);

        // Parse the version number
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

    eprintln!("✓ {} image SSH test passed", image_type);
}
