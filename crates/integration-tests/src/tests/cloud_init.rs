//! Integration tests for cloud-init ConfigDrive functionality
//!
//! These tests verify:
//! - ConfigDrive generation from user-provided cloud-config files
//! - ConfigDrive device creation and accessibility
//! - ConfigDrive content structure (OpenStack format)
//! - Kernel cmdline does NOT contain `ds=iid-datasource-none` when using ConfigDrive
//! - Cloud-init processing of the ConfigDrive (using localhost/bootc-cloud-init image)

use color_eyre::eyre::Context as _;
use color_eyre::Result;
use integration_tests::integration_test;

use crate::{run_bcvk, INTEGRATION_TEST_LABEL};

/// Get the cloud-init test image (built from tests/fixtures/cloud-init/)
fn get_cloud_init_test_image() -> String {
    std::env::var("BCVK_CLOUD_INIT_TEST_IMAGE")
        .unwrap_or_else(|_| "localhost/bootc-cloud-init".to_string())
}

/// Test basic cloud-init ConfigDrive functionality
///
/// Creates a cloud-config file, runs an ephemeral VM with --cloud-init,
/// and verifies that:
/// - The ConfigDrive device exists at /dev/disk/by-id/virtio-config-2
/// - The ConfigDrive can be mounted and contains expected OpenStack structure
/// - The user_data file contains the cloud-config content
/// - The meta_data.json contains the instance-id
fn test_cloud_init_configdrive_basic() -> Result<()> {
    let test_image = get_cloud_init_test_image();

    println!("Testing basic cloud-init ConfigDrive functionality");

    // Create a temporary cloud-config file
    let cloud_config_dir = tempfile::tempdir().context("Failed to create temp directory")?;
    let cloud_config_path = cloud_config_dir
        .path()
        .join("cloud-config.yaml")
        .to_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("Invalid UTF-8 in temp path"))?
        .to_string();

    // Create a simple cloud-config with identifiable content
    let cloud_config_content = r#"#cloud-config
write_files:
  - path: /tmp/test-marker
    content: |
      ConfigDrive test content
    permissions: '0644'

runcmd:
  - echo "Test command from cloud-config"
"#;

    std::fs::write(&cloud_config_path, cloud_config_content)
        .context("Failed to write cloud-config file")?;

    println!("Created cloud-config file at: {}", cloud_config_path);

    // Run ephemeral VM and verify ConfigDrive structure
    println!("Running ephemeral VM with --cloud-init...");
    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--cloud-init",
        &cloud_config_path,
        "--execute",
        "/bin/sh -c 'ls -la /dev/disk/by-id/virtio-config-2 && mkdir -p /mnt/configdrive && mount /dev/disk/by-id/virtio-config-2 /mnt/configdrive && ls -la /mnt/configdrive/ && cat /mnt/configdrive/openstack/latest/user_data && cat /mnt/configdrive/openstack/latest/meta_data.json'",
        &test_image,
    ])?;

    println!("VM execution completed");

    // Check the output
    println!("=== STDOUT ===");
    println!("{}", output.stdout);
    println!("=== STDERR ===");
    println!("{}", output.stderr);

    let combined_output = format!("{}\n{}", output.stdout, output.stderr);

    // Verify ConfigDrive device symlink exists
    assert!(
        combined_output.contains("virtio-config-2"),
        "ConfigDrive device symlink 'virtio-config-2' not found in output. Output: {}",
        combined_output
    );

    // Verify user_data contains the cloud-config header
    assert!(
        combined_output.contains("#cloud-config"),
        "user_data does not contain #cloud-config header. Output: {}",
        combined_output
    );

    // Verify user_data contains our test content
    assert!(
        combined_output.contains("ConfigDrive test content"),
        "user_data does not contain expected test content. Output: {}",
        combined_output
    );

    // Verify meta_data.json contains uuid (which cloud-init maps to instance-id)
    assert!(
        combined_output.contains("uuid"),
        "meta_data.json does not contain uuid. Output: {}",
        combined_output
    );

    // Also verify it contains the expected uuid value
    assert!(
        combined_output.contains("iid-local01"),
        "meta_data.json does not contain expected uuid value 'iid-local01'. Output: {}",
        combined_output
    );

    println!("✓ Basic cloud-init ConfigDrive test passed");
    output.assert_success("ephemeral run with cloud-init");
    Ok(())
}
integration_test!(test_cloud_init_configdrive_basic);

/// Test that kernel cmdline does NOT contain `ds=iid-datasource-none` when using ConfigDrive
///
/// When a ConfigDrive is provided, the kernel cmdline should NOT contain the
/// `ds=iid-datasource-none` parameter which would disable cloud-init.
/// This test verifies the cmdline directly without depending on cloud-init.
fn test_cloud_init_no_datasource_cmdline() -> Result<()> {
    let test_image = get_cloud_init_test_image();

    println!("Testing kernel cmdline does NOT contain ds=iid-datasource-none with ConfigDrive");

    // Create a temporary cloud-config file
    let cloud_config_dir = tempfile::tempdir().context("Failed to create temp directory")?;
    let cloud_config_path = cloud_config_dir
        .path()
        .join("cloud-config.yaml")
        .to_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("Invalid UTF-8 in temp path"))?
        .to_string();

    // Create a minimal cloud-config
    let cloud_config_content = r#"#cloud-config
runcmd:
  - echo "test"
"#;

    std::fs::write(&cloud_config_path, cloud_config_content)
        .context("Failed to write cloud-config file")?;

    println!("Created cloud-config file");

    // Run ephemeral VM and check /proc/cmdline directly
    println!("Running ephemeral VM to check kernel cmdline...");
    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--cloud-init",
        &cloud_config_path,
        "--execute",
        "cat /proc/cmdline",
        &test_image,
    ])?;

    println!("VM execution completed");
    println!("=== Output ===");
    println!("{}", output.stdout);

    // Get the kernel cmdline from the output
    let combined_output = format!("{}\n{}", output.stdout, output.stderr);

    // Verify that ds=iid-datasource-none is NOT present in the cmdline
    assert!(
        !combined_output.contains("ds=iid-datasource-none"),
        "Kernel cmdline should NOT contain 'ds=iid-datasource-none' when using ConfigDrive.\nOutput: {}",
        combined_output
    );

    println!("✓ Kernel cmdline does NOT contain ds=iid-datasource-none");
    output.assert_success("ephemeral run with cloud-init");
    Ok(())
}
integration_test!(test_cloud_init_no_datasource_cmdline);

/// Test that ConfigDrive contains expected user_data content
///
/// Creates a cloud-config with multiple runcmd directives,
/// then verifies the ConfigDrive user_data contains all expected content.
/// This test does NOT depend on cloud-init being installed - it directly
/// inspects the ConfigDrive contents.
fn test_cloud_init_configdrive_content() -> Result<()> {
    let test_image = get_cloud_init_test_image();

    println!("Testing ConfigDrive content verification");

    // Create a temporary cloud-config file
    let cloud_config_dir = tempfile::tempdir().context("Failed to create temp directory")?;
    let cloud_config_path = cloud_config_dir
        .path()
        .join("cloud-config.yaml")
        .to_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("Invalid UTF-8 in temp path"))?
        .to_string();

    // Create a cloud-config with multiple runcmd directives
    let cloud_config_content = r#"#cloud-config
runcmd:
  - echo "RUNCMD_TEST_1_SUCCESS"
  - echo "RUNCMD_TEST_2_SUCCESS"
  - echo "RUNCMD_TEST_3_SUCCESS"
"#;

    std::fs::write(&cloud_config_path, cloud_config_content)
        .context("Failed to write cloud-config file")?;

    println!("Created cloud-config with runcmd directives");

    // Run ephemeral VM and verify ConfigDrive user_data content
    println!("Running ephemeral VM to verify ConfigDrive content...");
    let output = run_bcvk(&[
        "ephemeral",
        "run",
        "--rm",
        "--label",
        INTEGRATION_TEST_LABEL,
        "--cloud-init",
        &cloud_config_path,
        "--execute",
        "/bin/sh -c 'mkdir -p /mnt && mount /dev/disk/by-id/virtio-config-2 /mnt && cat /mnt/openstack/latest/user_data'",
        &test_image,
    ])?;

    println!("VM execution completed");
    println!("=== Output ===");
    println!("{}", output.stdout);

    // Verify user_data contains all runcmd directives
    let combined_output = format!("{}\n{}", output.stdout, output.stderr);

    assert!(
        combined_output.contains("RUNCMD_TEST_1_SUCCESS"),
        "First runcmd directive not found in user_data. Output: {}",
        combined_output
    );

    assert!(
        combined_output.contains("RUNCMD_TEST_2_SUCCESS"),
        "Second runcmd directive not found in user_data. Output: {}",
        combined_output
    );

    assert!(
        combined_output.contains("RUNCMD_TEST_3_SUCCESS"),
        "Third runcmd directive not found in user_data. Output: {}",
        combined_output
    );

    println!("✓ All expected content found in ConfigDrive user_data");
    output.assert_success("ephemeral run with cloud-init configdrive content");
    Ok(())
}
integration_test!(test_cloud_init_configdrive_content);
