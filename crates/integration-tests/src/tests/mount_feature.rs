//! Integration tests for mount features
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

use camino::Utf8Path;
use color_eyre::Result;
use linkme::distributed_slice;
use std::fs;
use tempfile::TempDir;

use crate::{get_test_image, run_bcvk, IntegrationTest, INTEGRATION_TESTS, INTEGRATION_TEST_LABEL};

/// Create a systemd unit that verifies a mount exists and tests writability
fn create_mount_verify_unit(
    unit_path: &Utf8Path,
    mount_name: &str,
    expected_file: &str,
    expected_content: Option<&str>,
    readonly: bool,
) -> std::io::Result<()> {
    let (description, content_check, write_check) = if readonly {
        (
            format!("Verify read-only mount {mount_name} and poweroff"),
            format!("ExecStart=test -f /run/virtiofs-mnt-{mount_name}/{expected_file}"),
            format!("ExecStart=/bin/sh -c '! echo test-write > /run/virtiofs-mnt-{mount_name}/write-test.txt 2>/dev/null'"),
        )
    } else {
        let content = expected_content.expect("expected_content required for writable mounts");
        (
            format!("Verify mount {mount_name} and poweroff"),
            format!("ExecStart=grep -qF \"{content}\" /run/virtiofs-mnt-{mount_name}/{expected_file}"),
            format!("ExecStart=/bin/sh -c 'echo test-write > /run/virtiofs-mnt-{mount_name}/write-test.txt'"),
        )
    };

    let unit_content = format!(
        r#"[Unit]
Description={description}
RequiresMountsFor=/run/virtiofs-mnt-{mount_name}

[Service]
Type=oneshot
{content_check}
{write_check}
ExecStart=echo ok mount verify {mount_name}
ExecStart=systemctl poweroff
StandardOutput=journal+console
StandardError=journal+console

[Install]
WantedBy=default.target
"#
    );

    fs::write(unit_path, unit_content)?;
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_MOUNT_FEATURE_BIND: IntegrationTest =
    IntegrationTest::new("mount_feature_bind", test_mount_feature_bind);

fn test_mount_feature_bind() -> Result<()> {
    // Create a temporary directory to test bind mounting
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let temp_dir_path = Utf8Path::from_path(temp_dir.path()).expect("temp dir path is not utf8");
    let test_file_path = temp_dir_path.join("test.txt");
    let test_content = "Test content for bind mount";
    fs::write(&test_file_path, test_content).expect("Failed to write test file");

    // Create temporary unit file
    let unit_dir = TempDir::new().expect("Failed to create unit directory");
    let unit_dir_path = Utf8Path::from_path(unit_dir.path()).expect("unit dir path is not utf8");
    let unit_file = unit_dir_path.join("verify-mount-testmount.service");

    // Create verification unit
    create_mount_verify_unit(
        &unit_file,
        "testmount",
        "test.txt",
        Some(test_content),
        false,
    )
    .expect("Failed to create verify unit");

    println!("Testing bind mount with temp directory: {}", temp_dir_path);

    // Run with bind mount and verification unit
    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--console",
        "-K",
        "--bind",
        &format!("{}:testmount", temp_dir_path),
        "--add-unit",
        unit_file.as_str(),
        "--karg",
        "systemd.unit=verify-mount-testmount.service",
        "--karg",
        "systemd.journald.forward_to_console=1",
        &get_test_image(),
    ])?;

    assert!(output.stdout.contains("ok mount verify"));

    println!("Successfully tested and verified bind mount feature");
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_MOUNT_FEATURE_RO_BIND: IntegrationTest =
    IntegrationTest::new("mount_feature_ro_bind", test_mount_feature_ro_bind);

fn test_mount_feature_ro_bind() -> Result<()> {
    // Create a temporary directory to test read-only bind mounting
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let temp_dir_path = Utf8Path::from_path(temp_dir.path()).expect("temp dir path is not utf8");
    let test_file_path = temp_dir_path.join("readonly.txt");
    fs::write(&test_file_path, "Read-only content").expect("Failed to write test file");

    // Create temporary unit file
    let unit_dir = TempDir::new().expect("Failed to create unit directory");
    let unit_dir_path = Utf8Path::from_path(unit_dir.path()).expect("unit dir path is not utf8");
    let unit_file = unit_dir_path.join("verify-ro-mount-romount.service");

    // Create verification unit for read-only mount
    create_mount_verify_unit(&unit_file, "romount", "readonly.txt", None, true)
        .expect("Failed to create verify unit");

    println!(
        "Testing read-only bind mount with temp directory: {}",
        temp_dir_path
    );

    // Run with read-only bind mount and verification unit
    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--console",
        "-K",
        "--ro-bind",
        &format!("{}:romount", temp_dir_path),
        "--add-unit",
        unit_file.as_str(),
        "--karg",
        "systemd.unit=verify-ro-mount-romount.service",
        "--karg",
        "systemd.journald.forward_to_console=1",
        &get_test_image(),
    ])?;

    assert!(output.stdout.contains("ok mount verify"));
    Ok(())
}
