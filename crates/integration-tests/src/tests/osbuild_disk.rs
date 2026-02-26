//! Integration tests for osbuild-disk command
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

use camino::Utf8PathBuf;
use color_eyre::Result;
use linkme::distributed_slice;
use std::process::Command;
use tempfile::TempDir;

use crate::{run_bcvk, IntegrationTest, INTEGRATION_TESTS, INTEGRATION_TEST_LABEL};

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_OSBUILD_DISK_QCOW2: IntegrationTest =
    IntegrationTest::new("osbuild_disk_qcow2", test_osbuild_disk_qcow2);

/// Test building a qcow2 disk image with bootc-image-builder
fn test_osbuild_disk_qcow2() -> Result<()> {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let output_dir =
        Utf8PathBuf::try_from(temp_dir.path().to_path_buf()).expect("temp path is not UTF-8");

    let output = run_bcvk(&[
        "osbuild-disk",
        "--label",
        INTEGRATION_TEST_LABEL,
        "quay.io/centos-bootc/centos-bootc:stream10",
        output_dir.as_str(),
    ])
    .expect("Failed to run bcvk osbuild-disk");

    assert!(
        output.success(),
        "osbuild-disk failed with exit code: {:?}. stdout: {}, stderr: {}",
        output.exit_code(),
        output.stdout,
        output.stderr
    );

    // Verify output directory contains qcow2 subdirectory
    let qcow2_dir = output_dir.join("qcow2");
    assert!(
        qcow2_dir.exists(),
        "qcow2 output directory not found at {}",
        qcow2_dir
    );

    // Verify disk.qcow2 file exists
    let disk_path = qcow2_dir.join("disk.qcow2");
    assert!(
        disk_path.exists(),
        "disk.qcow2 file not found at {}",
        disk_path
    );

    let metadata = std::fs::metadata(&disk_path).expect("Failed to get disk metadata");
    assert!(metadata.len() > 0, "Disk image is empty");

    // Verify the file is actually qcow2 format using qemu-img info
    let qemu_img_output = Command::new("qemu-img")
        .args(["info", disk_path.as_str()])
        .output()
        .expect("Failed to run qemu-img info");

    let qemu_img_stdout = String::from_utf8_lossy(&qemu_img_output.stdout);

    assert!(
        qemu_img_output.status.success(),
        "qemu-img info failed with exit code: {:?}",
        qemu_img_output.status.code()
    );

    assert!(
        qemu_img_stdout.contains("file format: qcow2"),
        "qemu-img info doesn't show qcow2 format. Output was:\n{}",
        qemu_img_stdout
    );

    assert!(
        output.stdout.contains("Build completed successfully!")
            || output.stderr.contains("Build completed successfully!"),
        "No 'Build completed successfully!' message found in output. stdout: {}, stderr: {}",
        output.stdout,
        output.stderr
    );

    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_OSBUILD_DISK_WITH_CONFIG: IntegrationTest =
    IntegrationTest::new("osbuild_disk_with_config", test_osbuild_disk_with_config);

/// Test building with a custom config file
fn test_osbuild_disk_with_config() -> Result<()> {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let output_dir =
        Utf8PathBuf::try_from(temp_dir.path().join("output")).expect("temp path is not UTF-8");
    std::fs::create_dir_all(&output_dir).expect("Failed to create output directory");

    // Create a simple config file with user customization
    let config_path = temp_dir.path().join("config.toml");
    let config_content = r#"
[[customizations.user]]
name = "testuser"
password = "testpass"
groups = ["wheel"]
"#;
    std::fs::write(&config_path, config_content).expect("Failed to write config file");

    let config_path_str = config_path.to_str().expect("Config path is not UTF-8");

    let output = run_bcvk(&[
        "osbuild-disk",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--config",
        config_path_str,
        "quay.io/centos-bootc/centos-bootc:stream10",
        output_dir.as_str(),
    ])
    .expect("Failed to run bcvk osbuild-disk with config");

    assert!(
        output.success(),
        "osbuild-disk with config failed with exit code: {:?}. stdout: {}, stderr: {}",
        output.exit_code(),
        output.stdout,
        output.stderr
    );

    // Verify output directory contains qcow2 subdirectory
    let qcow2_dir = output_dir.join("qcow2");
    assert!(
        qcow2_dir.exists(),
        "qcow2 output directory not found at {}",
        qcow2_dir
    );

    // Verify disk.qcow2 file exists
    let disk_path = qcow2_dir.join("disk.qcow2");
    assert!(
        disk_path.exists(),
        "disk.qcow2 file not found at {}",
        disk_path
    );

    let metadata = std::fs::metadata(&disk_path).expect("Failed to get disk metadata");
    assert!(metadata.len() > 0, "Disk image is empty");

    assert!(
        output.stdout.contains("Build completed successfully!")
            || output.stderr.contains("Build completed successfully!"),
        "No 'Build completed successfully!' message found in output. stdout: {}, stderr: {}",
        output.stdout,
        output.stderr
    );

    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_OSBUILD_DISK_RAW: IntegrationTest =
    IntegrationTest::new("osbuild_disk_raw", test_osbuild_disk_raw);

/// Test building a raw disk image
fn test_osbuild_disk_raw() -> Result<()> {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let output_dir =
        Utf8PathBuf::try_from(temp_dir.path().to_path_buf()).expect("temp path is not UTF-8");

    let output = run_bcvk(&[
        "osbuild-disk",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--type",
        "raw",
        "quay.io/centos-bootc/centos-bootc:stream10",
        output_dir.as_str(),
    ])
    .expect("Failed to run bcvk osbuild-disk with raw format");

    assert!(
        output.success(),
        "osbuild-disk with raw format failed with exit code: {:?}. stdout: {}, stderr: {}",
        output.exit_code(),
        output.stdout,
        output.stderr
    );

    // Verify output directory contains image subdirectory (raw images go here)
    let image_dir = output_dir.join("image");
    assert!(
        image_dir.exists(),
        "image output directory not found at {}",
        image_dir
    );

    // Verify disk.raw file exists
    let disk_path = image_dir.join("disk.raw");
    assert!(
        disk_path.exists(),
        "disk.raw file not found at {}",
        disk_path
    );

    let metadata = std::fs::metadata(&disk_path).expect("Failed to get disk metadata");
    assert!(metadata.len() > 0, "Disk image is empty");

    // Verify the file is raw format using qemu-img info
    let qemu_img_output = Command::new("qemu-img")
        .args(["info", disk_path.as_str()])
        .output()
        .expect("Failed to run qemu-img info");

    let qemu_img_stdout = String::from_utf8_lossy(&qemu_img_output.stdout);

    assert!(
        qemu_img_output.status.success(),
        "qemu-img info failed with exit code: {:?}",
        qemu_img_output.status.code()
    );

    assert!(
        qemu_img_stdout.contains("file format: raw"),
        "qemu-img info doesn't show raw format. Output was:\n{}",
        qemu_img_stdout
    );

    assert!(
        output.stdout.contains("Build completed successfully!")
            || output.stderr.contains("Build completed successfully!"),
        "No 'Build completed successfully!' message found in output. stdout: {}, stderr: {}",
        output.stdout,
        output.stderr
    );

    Ok(())
}
