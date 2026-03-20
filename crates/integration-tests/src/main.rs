//! Integration tests for bcvk

use camino::Utf8Path;

use anyhow::{anyhow, Context};
use serde_json::Value;
use xshell::{cmd, Shell};

// Re-export from the lib crate for internal use
pub(crate) use integration_tests::{
    integration_test, INTEGRATION_TEST_LABEL, LIBVIRT_INTEGRATION_TEST_LABEL,
};

mod tests {
    pub mod libvirt_base_disks;
    pub mod libvirt_port_forward;
    pub mod libvirt_upload_disk;
    pub mod libvirt_verb;
    pub mod mount_feature;
    pub mod run_ephemeral;
    pub mod run_ephemeral_ignition;
    pub mod run_ephemeral_ssh;
    pub mod to_disk;
    pub mod varlink;
}

/// Create a new xshell Shell for running commands
pub(crate) fn shell() -> anyhow::Result<Shell> {
    Shell::new().map_err(|e| anyhow!("Failed to create shell: {}", e))
}

/// Get the path to the bcvk binary, checking BCVK_PATH env var first, then falling back to "bcvk"
pub(crate) fn get_bck_command() -> anyhow::Result<String> {
    if let Ok(path) = std::env::var("BCVK_PATH") {
        return Ok(path);
    }
    // Force the user to set this if we're running from the project dir
    if let Some(path) = ["target/debug/bcvk", "target/release/bcvk"]
        .into_iter()
        .find(|p| Utf8Path::new(p).exists())
    {
        return Err(anyhow!(
            "Detected {path} - set BCVK_PATH={path} to run using this binary"
        ));
    }
    Ok("bcvk".to_owned())
}

/// Get the primary bootc image to use for tests
///
/// Checks BCVK_PRIMARY_IMAGE environment variable first, then falls back to BCVK_TEST_IMAGE
/// for backwards compatibility, then to a hardcoded default.
pub(crate) fn get_test_image() -> String {
    std::env::var("BCVK_PRIMARY_IMAGE")
        .or_else(|_| std::env::var("BCVK_TEST_IMAGE"))
        .unwrap_or_else(|_| "quay.io/centos-bootc/centos-bootc:stream10".to_string())
}

/// Get all test images for matrix testing
///
/// Parses BCVK_ALL_IMAGES environment variable, which should be a whitespace-separated
/// list of container images (spaces, tabs, and newlines are all acceptable separators).
/// Falls back to a single-element vec containing the primary image if not set or empty.
///
/// Example: `export BCVK_ALL_IMAGES="quay.io/fedora/fedora-bootc:42 quay.io/centos-bootc/centos-bootc:stream9"`
pub(crate) fn get_all_test_images() -> Vec<String> {
    if let Ok(all_images) = std::env::var("BCVK_ALL_IMAGES") {
        let images: Vec<String> = all_images
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();

        if images.is_empty() {
            eprintln!("Warning: BCVK_ALL_IMAGES is set but empty, falling back to primary image");
            vec![get_test_image()]
        } else {
            images
        }
    } else {
        vec![get_test_image()]
    }
}

fn test_images_list() -> itest::TestResult {
    println!("Running test: bcvk images list --json");

    let sh = shell()?;
    let bck = get_bck_command()?;

    // Run the bcvk images list command with JSON output
    let stdout = cmd!(sh, "{bck} images list --json").read()?;

    // Parse the JSON output
    let images: Value = serde_json::from_str(&stdout).context("Failed to parse JSON output")?;

    // Verify the structure and content of the JSON
    let images_array = images
        .as_array()
        .ok_or_else(|| anyhow!("Expected JSON array in output, got: {}", stdout))?;

    // Verify that the array contains valid image objects
    for (index, image) in images_array.iter().enumerate() {
        if !image.is_object() {
            return Err(anyhow!("Image entry {} is not a JSON object: {}", index, image).into());
        }
    }

    println!(
        "Test passed: bck images list --json (found {} images)",
        images_array.len()
    );
    println!("All image entries are valid JSON objects");
    Ok(())
}
integration_test!(test_images_list);

fn main() {
    let config = itest::TestConfig {
        report_name: "bcvk-integration-tests".into(),
        suite_name: "integration".into(),
        parameters: get_all_test_images(),
    };

    itest::run_tests_with_config(config);
}
