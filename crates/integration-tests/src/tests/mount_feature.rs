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
use integration_tests::integration_test;

use std::fs;
use tempfile::TempDir;

use xshell::cmd;

use crate::{get_bck_command, get_test_image, shell, INTEGRATION_TEST_LABEL};

/// Create a systemd unit that verifies a mount exists and tests writability
fn create_mount_verify_unit(
    unit_dir: &Utf8Path,
    mount_name: &str,
    expected_file: &str,
    expected_content: Option<&str>,
    readonly: bool,
) -> std::io::Result<()> {
    let (description, content_check, write_check, unit_prefix) = if readonly {
        (
            format!("Verify read-only mount {mount_name} and poweroff"),
            format!("ExecStart=test -f /run/virtiofs-mnt-{mount_name}/{expected_file}"),
            format!("ExecStart=/bin/sh -c '! echo test-write > /run/virtiofs-mnt-{mount_name}/write-test.txt 2>/dev/null'"),
            "verify-ro-mount",
        )
    } else {
        let content = expected_content.expect("expected_content required for writable mounts");
        (
            format!("Verify mount {mount_name} and poweroff"),
            format!("ExecStart=grep -qF \"{content}\" /run/virtiofs-mnt-{mount_name}/{expected_file}"),
            format!("ExecStart=/bin/sh -c 'echo test-write > /run/virtiofs-mnt-{mount_name}/write-test.txt'"),
            "verify-mount",
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
"#
    );

    let unit_path = unit_dir.join(format!("{unit_prefix}-{mount_name}.service"));
    fs::write(&unit_path, unit_content)?;
    Ok(())
}

fn test_mount_feature_bind() -> Result<()> {
    // Create a temporary directory to test bind mounting
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let temp_dir_path = Utf8Path::from_path(temp_dir.path()).expect("temp dir path is not utf8");
    let test_file_path = temp_dir_path.join("test.txt");
    let test_content = "Test content for bind mount";
    fs::write(&test_file_path, test_content).expect("Failed to write test file");

    // Create systemd units directory
    let units_dir = TempDir::new().expect("Failed to create units directory");
    let units_dir_path = Utf8Path::from_path(units_dir.path()).expect("units dir path is not utf8");
    let system_dir = units_dir_path.join("system");
    fs::create_dir(&system_dir).expect("Failed to create system directory");

    // Create verification unit
    create_mount_verify_unit(
        &system_dir,
        "testmount",
        "test.txt",
        Some(test_content),
        false,
    )
    .expect("Failed to create verify unit");

    println!("Testing bind mount with temp directory: {}", temp_dir_path);

    let sh = shell()?;
    let bck = get_bck_command()?;
    let label = INTEGRATION_TEST_LABEL;
    let image = get_test_image();
    let bind_arg = format!("{}:testmount", temp_dir_path);

    // Run with bind mount and verification unit
    let stdout = cmd!(
        sh,
        "{bck} ephemeral run --rm --label {label} --console -K --bind {bind_arg} --systemd-units {units_dir_path} --karg systemd.unit=verify-mount-testmount.service --karg systemd.journald.forward_to_console=1 {image}"
    )
    .read()?;

    assert!(stdout.contains("ok mount verify"));

    println!("Successfully tested and verified bind mount feature");
    Ok(())
}
integration_test!(test_mount_feature_bind);

fn test_mount_feature_ro_bind() -> Result<()> {
    // Create a temporary directory to test read-only bind mounting
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let temp_dir_path = Utf8Path::from_path(temp_dir.path()).expect("temp dir path is not utf8");
    let test_file_path = temp_dir_path.join("readonly.txt");
    fs::write(&test_file_path, "Read-only content").expect("Failed to write test file");

    // Create systemd units directory
    let units_dir = TempDir::new().expect("Failed to create units directory");
    let units_dir_path = Utf8Path::from_path(units_dir.path()).expect("units dir path is not utf8");
    let system_dir = units_dir_path.join("system");
    fs::create_dir(&system_dir).expect("Failed to create system directory");

    // Create verification unit for read-only mount
    create_mount_verify_unit(&system_dir, "romount", "readonly.txt", None, true)
        .expect("Failed to create verify unit");

    println!(
        "Testing read-only bind mount with temp directory: {}",
        temp_dir_path
    );

    let sh = shell()?;
    let bck = get_bck_command()?;
    let label = INTEGRATION_TEST_LABEL;
    let image = get_test_image();
    let ro_bind_arg = format!("{}:romount", temp_dir_path);

    // Run with read-only bind mount and verification unit
    let stdout = cmd!(
        sh,
        "{bck} ephemeral run --rm --label {label} --console -K --ro-bind {ro_bind_arg} --systemd-units {units_dir_path} --karg systemd.unit=verify-ro-mount-romount.service --karg systemd.journald.forward_to_console=1 {image}"
    )
    .read()?;

    assert!(stdout.contains("ok mount verify"));
    Ok(())
}
integration_test!(test_mount_feature_ro_bind);
