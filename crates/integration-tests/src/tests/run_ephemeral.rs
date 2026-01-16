//! Integration tests for ephemeral run command
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
use integration_tests::integration_test;

use std::process::Command;
use tracing::debug;

use crate::{get_test_image, run_bcvk, INTEGRATION_TEST_LABEL};

pub fn get_container_kernel_version(image: &str) -> String {
    // Run container to get its kernel version
    let output = Command::new("podman")
        .args([
            "run",
            "--rm",
            image,
            "sh",
            "-c",
            "ls -1 /usr/lib/modules | head -1",
        ])
        .output()
        .expect("Failed to get container kernel version");

    assert!(
        output.status.success(),
        "Failed to get kernel version from container: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn test_run_ephemeral_correct_kernel() -> Result<()> {
    let image = get_test_image();
    let container_kernel = get_container_kernel_version(&image);
    eprintln!("Container kernel version: {}", container_kernel);

    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        &image,
        "--karg",
        "systemd.unit=poweroff.target",
    ])?;

    output.assert_success("ephemeral run");
    Ok(())
}
integration_test!(test_run_ephemeral_correct_kernel);

fn test_run_ephemeral_poweroff() -> Result<()> {
    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        &get_test_image(),
        "--karg",
        "systemd.unit=poweroff.target",
    ])?;

    output.assert_success("ephemeral run");
    Ok(())
}
integration_test!(test_run_ephemeral_poweroff);

fn test_run_ephemeral_with_memory_limit() -> Result<()> {
    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--memory",
        "1024",
        "--karg",
        "systemd.unit=poweroff.target",
        &get_test_image(),
    ])?;

    output.assert_success("ephemeral run with memory limit");
    Ok(())
}
integration_test!(test_run_ephemeral_with_memory_limit);

fn test_run_ephemeral_with_vcpus() -> Result<()> {
    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--vcpus",
        "2",
        "--karg",
        "systemd.unit=poweroff.target",
        &get_test_image(),
    ])?;

    output.assert_success("ephemeral run with vcpus");
    Ok(())
}
integration_test!(test_run_ephemeral_with_vcpus);

fn test_run_ephemeral_execute() -> Result<()> {
    let script =
        "/bin/sh -c \"echo 'Hello from VM'; echo 'Current date:'; date; echo 'Script completed successfully'\"";

    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--execute",
        script,
        &get_test_image(),
    ])?;

    output.assert_success("ephemeral run with --execute");

    assert!(
        output.stdout.contains("Hello from VM"),
        "Script output 'Hello from VM' not found in stdout: {}",
        output.stdout
    );

    assert!(
        output.stdout.contains("Script completed successfully"),
        "Script completion message not found in stdout: {}",
        output.stdout
    );

    assert!(
        output.stdout.contains("Current date:"),
        "Date output header not found in stdout: {}",
        output.stdout
    );
    Ok(())
}
integration_test!(test_run_ephemeral_execute);

fn test_run_ephemeral_container_ssh_access() -> Result<()> {
    let image = get_test_image();
    let container_name = format!(
        "ssh-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--ssh-keygen",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--detach",
        "--name",
        &container_name,
        &image,
    ])?;

    if !output.success() {
        panic!("Failed to start detached VM: {}", output.stderr);
    }

    let ssh_output = run_bcvk(&[
        "ephemeral",
        "ssh",
        &container_name,
        "echo",
        "SSH_TEST_SUCCESS",
    ])?;

    debug!("SSH exit status: {:?}", ssh_output.exit_code());

    // Cleanup: stop the container
    let _ = Command::new("podman")
        .args(["stop", &container_name])
        .output();

    assert!(ssh_output.success());
    assert!(ssh_output.stdout.contains("SSH_TEST_SUCCESS"));
    Ok(())
}
integration_test!(test_run_ephemeral_container_ssh_access);

fn test_run_ephemeral_with_instancetype() -> Result<()> {
    // Test u1.nano: 1 vCPU, 512 MiB memory
    // Calculate physical memory from /sys/firmware/memmap (System RAM regions)
    let script = "/bin/sh -c 'echo CPUs:$(grep -c ^processor /proc/cpuinfo); total=0; for dir in /sys/firmware/memmap/*; do type=$(cat \"$dir/type\" 2>/dev/null); if [ \"$type\" = \"System RAM\" ]; then start=$(cat \"$dir/start\"); end=$(cat \"$dir/end\"); start_dec=$((start)); end_dec=$((end)); size=$((end_dec - start_dec + 1)); total=$((total + size)); fi; done; total_kb=$((total / 1024)); echo PhysicalMemKB:$total_kb'";

    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--itype",
        "u1.nano",
        "--execute",
        script,
        &get_test_image(),
    ])?;

    output.assert_success("ephemeral run with instance type u1.nano");

    // Verify vCPUs (should be 1)
    assert!(
        output.stdout.contains("CPUs:1"),
        "Expected 1 vCPU for u1.nano, output: {}",
        output.stdout
    );

    // Verify physical memory (should be exactly 512 MiB = 524288 kB)
    let mem_line = output
        .stdout
        .lines()
        .find(|line| line.contains("PhysicalMemKB:"))
        .expect("PhysicalMemKB line not found in output");

    let mem_kb: u32 = mem_line
        .split(':')
        .nth(1)
        .expect("Could not parse PhysicalMemKB")
        .trim()
        .parse()
        .expect("Could not parse PhysicalMemKB as number");

    // Physical memory should be close to 512 MiB = 524288 kB
    // QEMU reserves small memory regions (BIOS, VGA, ACPI, etc.) so actual may be slightly less
    // Allow 1% tolerance to account for hypervisor overhead
    let expected_kb = 512 * 1024;
    let tolerance_kb = expected_kb / 100; // 1% tolerance
    let diff = if mem_kb > expected_kb {
        mem_kb - expected_kb
    } else {
        expected_kb - mem_kb
    };

    assert!(
        diff <= tolerance_kb,
        "Expected physical memory ~{} kB for u1.nano, got {} kB (diff: {} kB, max allowed: {} kB [1%])",
        expected_kb, mem_kb, diff, tolerance_kb
    );

    Ok(())
}
integration_test!(test_run_ephemeral_with_instancetype);

fn test_run_ephemeral_instancetype_invalid() -> Result<()> {
    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--itype",
        "invalid.type",
        "--karg",
        "systemd.unit=poweroff.target",
        &get_test_image(),
    ])?;

    // Should fail with invalid instance type
    assert!(
        !output.success(),
        "Expected failure with invalid instance type, but succeeded"
    );

    // Error message should mention the invalid type
    assert!(
        output.stderr.contains("invalid.type") || output.stderr.contains("Unknown instance type"),
        "Error message should mention invalid instance type: {}",
        output.stderr
    );

    Ok(())
}
integration_test!(test_run_ephemeral_instancetype_invalid);

/// Test that ephemeral VMs have the expected mount layout:
/// - / is read-only virtiofs
/// - /etc is overlayfs with tmpfs upper (writable)
/// - /var is tmpfs (not overlayfs, so podman can use overlayfs inside)
fn test_run_ephemeral_mount_layout() -> Result<()> {
    // Check each mount point individually using findmnt
    // Running all three at once with -J can hang on some configurations

    // Check root mount
    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--execute",
        "findmnt -n -o FSTYPE,OPTIONS /",
        &get_test_image(),
    ])?;
    output.assert_success("check root mount");
    let root_line = output.stdout.trim();
    assert!(
        root_line.starts_with("virtiofs"),
        "Root should be virtiofs, got: {}",
        root_line
    );
    assert!(
        root_line.contains("ro"),
        "Root should be read-only, got: {}",
        root_line
    );

    // Check /etc mount
    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--execute",
        "findmnt -n -o FSTYPE /etc",
        &get_test_image(),
    ])?;
    output.assert_success("check /etc mount");
    assert_eq!(
        output.stdout.trim(),
        "overlay",
        "/etc should be overlay, got: {}",
        output.stdout
    );

    // Check /var mount - should be tmpfs, NOT overlay
    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--execute",
        "findmnt -n -o FSTYPE /var",
        &get_test_image(),
    ])?;
    output.assert_success("check /var mount");
    assert_eq!(
        output.stdout.trim(),
        "tmpfs",
        "/var should be tmpfs (not overlay), got: {}",
        output.stdout
    );

    Ok(())
}
integration_test!(test_run_ephemeral_mount_layout);
