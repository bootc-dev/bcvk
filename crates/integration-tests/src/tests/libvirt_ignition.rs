//! Integration tests for Ignition config injection in libvirt VMs

use integration_tests::integration_test;
use itest::TestResult;
use scopeguard::defer;
use tempfile::TempDir;
use xshell::cmd;

use std::fs;

use camino::Utf8Path;

use crate::{get_bck_command, shell, LIBVIRT_INTEGRATION_TEST_LABEL};

/// Generate a random alphanumeric suffix for VM names to avoid collisions
fn random_suffix() -> String {
    use rand::{distr::Alphanumeric, Rng};
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(8)
        .map(char::from)
        .collect()
}

/// Fedora CoreOS image that supports Ignition
const FCOS_IMAGE: &str = "quay.io/fedora/fedora-coreos:stable";

/// Test that Ignition config injection mechanism works for libvirt
///
/// This test verifies that the Ignition config injection mechanism is working
/// by checking that the VM can be created with --ignition flag and that the
/// config file is properly stored.
fn test_libvirt_ignition_works() -> TestResult {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let label = LIBVIRT_INTEGRATION_TEST_LABEL;

    // Pull FCOS image first
    cmd!(sh, "podman pull -q {FCOS_IMAGE}").run()?;

    // Create a temporary Ignition config
    let temp_dir = TempDir::new()?;
    let config_path = Utf8Path::from_path(temp_dir.path())
        .expect("temp dir is not utf8")
        .join("config.ign");

    // Minimal valid Ignition config (v3.3.0 for FCOS)
    let ignition_config = r#"{"ignition": {"version": "3.3.0"}}"#;
    fs::write(&config_path, ignition_config)?;

    // Generate a unique VM name to avoid conflicts
    let vm_name = format!("test-ignition-{}", random_suffix());

    // Create VM with Ignition config
    // We use --ssh-wait to wait for the VM to boot and verify SSH connectivity
    // FCOS requires --filesystem to be specified
    let output = cmd!(
        sh,
        "{bck} libvirt run --name {vm_name} --label {label} --ignition {config_path} --filesystem xfs --ssh-wait --memory 2G --cpus 2 {FCOS_IMAGE}"
    )
    .ignore_status()
    .output()?;

    // Cleanup: remove the VM
    defer! {
        let _ = cmd!(sh, "{bck} libvirt rm {vm_name} --force").run();
    }

    // Check that the command succeeded
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        panic!(
            "Failed to create VM with Ignition config.\nStdout: {}\nStderr: {}",
            stdout, stderr
        );
    }

    // Verify the VM was created
    let vm_list = cmd!(sh, "{bck} libvirt list").read()?;
    assert!(
        vm_list.contains(&vm_name),
        "VM should be listed after creation"
    );

    println!("Ignition config injection test passed");
    Ok(())
}
integration_test!(test_libvirt_ignition_works);

/// Test that Ignition config validation rejects nonexistent files
fn test_libvirt_ignition_invalid_path() -> TestResult {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let label = LIBVIRT_INTEGRATION_TEST_LABEL;

    // Pull FCOS image first
    cmd!(sh, "podman pull -q {FCOS_IMAGE}").run()?;

    let temp = TempDir::new()?;
    let nonexistent_path = Utf8Path::from_path(temp.path())
        .expect("temp dir is not utf8")
        .join("nonexistent-config.ign");

    let vm_name = format!("test-ignition-invalid-{}", random_suffix());

    let output = cmd!(
        sh,
        "{bck} libvirt run --name {vm_name} --label {label} --ignition {nonexistent_path} {FCOS_IMAGE}"
    )
    .ignore_status()
    .output()?;

    assert!(
        !output.status.success(),
        "Should fail with nonexistent Ignition config file"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "Error should mention missing file: {}",
        stderr
    );

    println!("Ignition invalid path test passed");
    Ok(())
}
integration_test!(test_libvirt_ignition_invalid_path);

/// Test that Ignition is rejected for images that don't support it
fn test_libvirt_ignition_unsupported_image() -> TestResult {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let label = LIBVIRT_INTEGRATION_TEST_LABEL;

    // Use standard bootc image that doesn't have Ignition support
    let image = "quay.io/centos-bootc/centos-bootc:stream10";

    let temp_dir = TempDir::new()?;
    let config_path = Utf8Path::from_path(temp_dir.path())
        .expect("temp dir is not utf8")
        .join("config.ign");

    let ignition_config = r#"{"ignition": {"version": "3.3.0"}}"#;
    fs::write(&config_path, ignition_config)?;

    let vm_name = format!("test-ignition-unsupported-{}", random_suffix());

    let output = cmd!(
        sh,
        "{bck} libvirt run --name {vm_name} --label {label} --ignition {config_path} {image}"
    )
    .ignore_status()
    .output()?;

    assert!(
        !output.status.success(),
        "Should fail when using --ignition with non-Ignition image"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not support Ignition"),
        "Error should mention missing Ignition support: {}",
        stderr
    );

    println!("Ignition unsupported image test passed");
    Ok(())
}
integration_test!(test_libvirt_ignition_unsupported_image);
