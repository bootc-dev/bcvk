//! Integration tests for to-disk command
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

use std::process::Output;

use camino::Utf8PathBuf;
use color_eyre::Result;
use integration_tests::{integration_test, parameterized_integration_test};
use xshell::cmd;

use tempfile::TempDir;

use crate::{get_bck_command, get_test_image, shell, INTEGRATION_TEST_LABEL};

/// Validate that a disk image was created successfully with proper bootc installation
///
/// This helper function verifies:
/// - The disk image file exists and has non-zero size
/// - The disk has valid partition table (using sfdisk, only for raw images)
/// - The installation completed successfully (from output messages)
///
/// Note: sfdisk can only read partition tables from raw disk images, not qcow2.
/// For qcow2 images, partition validation is skipped.
fn validate_disk_image(disk_path: &Utf8PathBuf, output: &Output, context: &str) -> Result<()> {
    let metadata = std::fs::metadata(disk_path).expect("Failed to get disk metadata");
    assert!(metadata.len() > 0, "{}: Disk image is empty", context);

    // Only verify partitions for raw images - sfdisk can't read qcow2 format
    let is_qcow2 = disk_path.as_str().ends_with(".qcow2");
    if !is_qcow2 {
        // Verify the disk has partitions using sfdisk -l
        let sh = shell().expect("Failed to create shell");
        let sfdisk_stdout = cmd!(sh, "sfdisk -l {disk_path}").read()?;

        assert!(
            sfdisk_stdout.contains("Disk ")
                && (sfdisk_stdout.contains("sectors") || sfdisk_stdout.contains("bytes")),
            "{}: sfdisk output doesn't show expected disk information",
            context
        );

        let has_partitions = sfdisk_stdout.lines().any(|line| {
            line.contains(disk_path.as_str()) && (line.contains("Linux") || line.contains("EFI"))
        });

        assert!(
            has_partitions,
            "{}: No bootc partitions found in sfdisk output. Output was:\n{}",
            context, sfdisk_stdout
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("Installation complete") || stderr.contains("Installation complete"),
        "{}: No 'Installation complete' message found in output. This indicates bootc install did not complete successfully. stdout: {}, stderr: {}",
        context,
        stdout, stderr
    );

    Ok(())
}

/// Test actual bootc installation to a disk image
fn test_to_disk() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let label = INTEGRATION_TEST_LABEL;
    let image = get_test_image();

    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let disk_path = Utf8PathBuf::try_from(temp_dir.path().join("test-disk.img"))
        .expect("temp path is not UTF-8");

    let output = cmd!(sh, "{bck} to-disk --label {label} {image} {disk_path}").output()?;
    validate_disk_image(&disk_path, &output, "test_to_disk")?;
    Ok(())
}
integration_test!(test_to_disk);

/// Test bootc installation to a qcow2 disk image
fn test_to_disk_qcow2() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let label = INTEGRATION_TEST_LABEL;
    let image = get_test_image();

    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let disk_path = Utf8PathBuf::try_from(temp_dir.path().join("test-disk.qcow2"))
        .expect("temp path is not UTF-8");

    let output = cmd!(
        sh,
        "{bck} to-disk --format=qcow2 --label {label} {image} {disk_path}"
    )
    .output()?;

    // Verify the file is actually qcow2 format using qemu-img info
    let qemu_img_stdout = cmd!(sh, "qemu-img info {disk_path}").read()?;

    assert!(
        qemu_img_stdout.contains("file format: qcow2"),
        "qemu-img info doesn't show qcow2 format. Output was:\n{}",
        qemu_img_stdout
    );

    validate_disk_image(&disk_path, &output, "test_to_disk_qcow2")?;
    Ok(())
}
integration_test!(test_to_disk_qcow2);

/// Test disk image caching functionality
fn test_to_disk_caching() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let label = INTEGRATION_TEST_LABEL;
    let image = get_test_image();

    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let disk_path = Utf8PathBuf::try_from(temp_dir.path().join("test-disk-cache.img"))
        .expect("temp path is not UTF-8");

    // First run: Create the disk image
    let output1 = cmd!(sh, "{bck} to-disk --label {label} {image} {disk_path}").output()?;
    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    let stderr1 = String::from_utf8_lossy(&output1.stderr);

    let metadata1 =
        std::fs::metadata(&disk_path).expect("Failed to get disk metadata after first run");
    assert!(metadata1.len() > 0, "Disk image is empty after first run");

    assert!(
        stdout1.contains("Installation complete") || stderr1.contains("Installation complete"),
        "No 'Installation complete' message found in first run output"
    );

    // Second run: Should reuse the cached disk
    let output2 = cmd!(sh, "{bck} to-disk --label {label} {image} {disk_path}").output()?;
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    let stderr2 = String::from_utf8_lossy(&output2.stderr);

    assert!(
        stdout2.contains("Reusing existing cached disk image"),
        "Second run should have reused cached disk, but cache reuse message not found. stdout: {}, stderr: {}",
        stdout2, stderr2
    );

    let metadata2 =
        std::fs::metadata(&disk_path).expect("Failed to get disk metadata after second run");
    assert_eq!(
        metadata1.len(),
        metadata2.len(),
        "Disk size changed between runs, indicating it was recreated instead of reused"
    );

    assert!(
        !stdout2.contains("Installation complete") && !stderr2.contains("Installation complete"),
        "Second run should not have performed installation, but found 'Installation complete' message"
    );
    Ok(())
}
integration_test!(test_to_disk_caching);

/// Test that different image references with the same digest create separate cached disks
fn test_to_disk_different_imgref_same_digest() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let label = INTEGRATION_TEST_LABEL;
    let temp_dir = TempDir::new().expect("Failed to create temp directory");

    // First, pull the test image
    let test_image = get_test_image();
    cmd!(sh, "podman pull {test_image}").run()?;

    // Create a second tag pointing to the same digest
    let second_tag = format!("{}-alias", test_image);
    cmd!(sh, "podman tag {test_image} {second_tag}").run()?;

    // Create first disk with original image reference
    let disk_path = Utf8PathBuf::try_from(temp_dir.path().join("test-disk.img"))
        .expect("temp path is not UTF-8");

    cmd!(sh, "{bck} to-disk --label {label} {test_image} {disk_path}").run()?;

    let metadata1 =
        std::fs::metadata(&disk_path).expect("Failed to get disk metadata after first run");
    assert!(metadata1.len() > 0, "Disk image is empty");

    // Use --dry-run with the aliased image reference (same digest, different imgref)
    // to verify it would regenerate instead of reusing the cache
    let output2 = cmd!(
        sh,
        "{bck} to-disk --dry-run --label {label} {second_tag} {disk_path}"
    )
    .output()?;
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    let stderr2 = String::from_utf8_lossy(&output2.stderr);

    // The dry-run should report it would regenerate because the source imgref is different
    assert!(
        stdout2.contains("would-regenerate"),
        "Dry-run should report 'would-regenerate' for different imgref. stdout: {}, stderr: {}",
        stdout2,
        stderr2
    );

    // Clean up: remove the aliased tag
    let _ = cmd!(sh, "podman rmi {second_tag}")
        .ignore_status()
        .quiet()
        .run();

    Ok(())
}
integration_test!(test_to_disk_different_imgref_same_digest);

/// Test to-disk with various bootc images to ensure compatibility
///
/// This parameterized test runs to-disk with multiple container images,
/// particularly testing AlmaLinux which had cross-device link issues (issue #125)
fn test_to_disk_for_image(image: &str) -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let label = INTEGRATION_TEST_LABEL;

    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let disk_path = Utf8PathBuf::try_from(temp_dir.path().join("test-disk.img"))
        .expect("temp path is not UTF-8");

    // Not all images have a default filesystem, so explicitly specify ext4
    let output = cmd!(
        sh,
        "{bck} to-disk --label {label} --filesystem=ext4 {image} {disk_path}"
    )
    .output()?;

    validate_disk_image(
        &disk_path,
        &output,
        &format!("test_to_disk_multi_image({})", image),
    )?;
    Ok(())
}
parameterized_integration_test!(test_to_disk_for_image);
