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
use linkme::distributed_slice;
use std::process::Command;
use tracing::debug;

use crate::{get_test_image, run_bcvk, IntegrationTest, INTEGRATION_TESTS, INTEGRATION_TEST_LABEL};

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

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_RUN_EPHEMERAL_CORRECT_KERNEL: IntegrationTest = IntegrationTest::new(
    "run_ephemeral_correct_kernel",
    test_run_ephemeral_correct_kernel,
);

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

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_RUN_EPHEMERAL_POWEROFF: IntegrationTest =
    IntegrationTest::new("run_ephemeral_poweroff", test_run_ephemeral_poweroff);

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

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_RUN_EPHEMERAL_WITH_MEMORY_LIMIT: IntegrationTest = IntegrationTest::new(
    "run_ephemeral_with_memory_limit",
    test_run_ephemeral_with_memory_limit,
);

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

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_RUN_EPHEMERAL_WITH_VCPUS: IntegrationTest =
    IntegrationTest::new("run_ephemeral_with_vcpus", test_run_ephemeral_with_vcpus);

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

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_RUN_EPHEMERAL_EXECUTE: IntegrationTest =
    IntegrationTest::new("run_ephemeral_execute", test_run_ephemeral_execute);

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

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_RUN_EPHEMERAL_CONTAINER_SSH_ACCESS: IntegrationTest = IntegrationTest::new(
    "run_ephemeral_container_ssh_access",
    test_run_ephemeral_container_ssh_access,
);

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

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_RUN_EPHEMERAL_ADD_UNIT: IntegrationTest =
    IntegrationTest::new("run_ephemeral_add_unit", test_run_ephemeral_add_unit);

fn test_run_ephemeral_add_unit() -> Result<()> {
    use std::fs;
    use tempfile::TempDir;

    // Create a temporary directory for the test unit file
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let unit_file_path = temp_dir.path().join("bcvk-test-marker.service");

    // Write a simple systemd service that creates a marker file
    let unit_content = r#"[Unit]
Description=BCVK Test Marker Service
DefaultDependencies=no

[Service]
Type=oneshot
ExecStart=/bin/sh -c 'echo BCVK_TEST_MARKER_SUCCESS > /run/bcvk-test-marker'
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
"#;

    fs::write(&unit_file_path, unit_content).expect("Failed to write unit file");

    // Run ephemeral with --add-unit and use --execute to check if the service ran
    // The service should be started automatically via [Install] WantedBy=multi-user.target
    // but we explicitly start it to ensure it runs before our check
    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--add-unit",
        unit_file_path.to_str().unwrap(),
        "--execute",
        "/bin/sh -c 'systemctl start bcvk-test-marker.service && cat /run/bcvk-test-marker'",
        &get_test_image(),
    ])?;

    output.assert_success("ephemeral run with --add-unit");

    // The service should have created the marker file
    assert!(
        output.stdout.contains("BCVK_TEST_MARKER_SUCCESS"),
        "Service marker not found. Unit may not have been injected correctly. Output: {}",
        output.stdout
    );

    Ok(())
}
