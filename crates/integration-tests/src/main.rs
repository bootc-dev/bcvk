//! Integration tests for bcvk

use camino::Utf8Path;
use std::process::Output;

use color_eyre::eyre::{eyre, Context};
use color_eyre::Result;
use libtest_mimic::{Arguments, Trial};
use serde_json::Value;
use xshell::{cmd, Shell};

// Re-export constants from lib for internal use
pub(crate) use integration_tests::{
    IntegrationTest, INTEGRATION_TESTS, INTEGRATION_TEST_LABEL, LIBVIRT_INTEGRATION_TEST_LABEL,
};
use linkme::distributed_slice;

mod tests {
    pub mod libvirt_base_disks;
    pub mod libvirt_port_forward;
    pub mod libvirt_upload_disk;
    pub mod libvirt_verb;
    pub mod mount_feature;
    pub mod run_ephemeral;
    pub mod run_ephemeral_ssh;
    pub mod to_disk;
}

/// Get the path to the bcvk binary, checking BCVK_PATH env var first, then falling back to "bcvk"
pub(crate) fn get_bck_command() -> Result<String> {
    if let Some(path) = std::env::var("BCVK_PATH").ok() {
        return Ok(path);
    }
    // Force the user to set this if we're running from the project dir
    if let Some(path) = ["target/debug/bcvk", "target/release/bcvk"]
        .into_iter()
        .find(|p| Utf8Path::new(p).exists())
    {
        return Err(eyre!(
            "Detected {path} - set BCVK_PATH={path} to run using this binary"
        ));
    }
    return Ok("bcvk".to_owned());
}

/// Get the default bootc image to use for tests
///
/// Checks BCVK_TEST_IMAGE environment variable first, then falls back to default.
/// This allows easily overriding the base image for all integration tests.
///
/// Default images:
/// - Primary: quay.io/fedora/fedora-bootc:42 (Fedora 42 with latest features)
/// - Alternative: quay.io/centos-bootc/centos-bootc:stream9 (CentOS Stream 9 for compatibility testing)
pub(crate) fn get_test_image() -> String {
    std::env::var("BCVK_TEST_IMAGE")
        .unwrap_or_else(|_| "quay.io/fedora/fedora-bootc:42".to_string())
}

/// Get an alternative bootc image for cross-platform testing
///
/// Returns a different image from the primary test image to test compatibility.
/// If BCVK_TEST_IMAGE is set to Fedora, returns CentOS Stream 9.
/// If BCVK_TEST_IMAGE is set to CentOS, returns Fedora.
pub(crate) fn get_alternative_test_image() -> String {
    let primary = get_test_image();
    if primary.contains("centos") {
        "quay.io/fedora/fedora-bootc:42".to_string()
    } else {
        "quay.io/centos-bootc/centos-bootc:stream9".to_string()
    }
}

/// Captured output from a command with decoded stdout/stderr strings
pub(crate) struct CapturedOutput {
    pub output: Output,
    pub stdout: String,
    pub stderr: String,
}

impl CapturedOutput {
    /// Create from a raw Output
    pub fn new(output: Output) -> Self {
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        Self {
            output,
            stdout,
            stderr,
        }
    }

    /// Assert that the command succeeded, printing debug info on failure
    pub fn assert_success(&self, context: &str) {
        assert!(
            self.output.status.success(),
            "{} failed: {}",
            context,
            self.stderr
        );
    }

    /// Get the exit code
    pub fn exit_code(&self) -> Option<i32> {
        self.output.status.code()
    }

    /// Check if the command succeeded
    pub fn success(&self) -> bool {
        self.output.status.success()
    }
}

/// Run a command, capturing output
pub(crate) fn run_command(program: &str, args: &[&str]) -> std::io::Result<CapturedOutput> {
    let output = std::process::Command::new(program).args(args).output()?;
    Ok(CapturedOutput::new(output))
}

/// Run the bcvk command, capturing output
pub(crate) fn run_bcvk(args: &[&str]) -> std::io::Result<CapturedOutput> {
    let bck = get_bck_command().expect("Failed to get bcvk command");
    run_command(&bck, args)
}

/// Run the bcvk command with inherited stdout/stderr (no capture)
/// Use this when you just need to verify the command succeeded without checking output
pub(crate) fn run_bcvk_nocapture(args: &[&str]) -> std::io::Result<()> {
    let bck = get_bck_command().expect("Failed to get bcvk command");
    let status = std::process::Command::new(&bck).args(args).status()?;
    assert!(
        status.success(),
        "bcvk command failed with args: {:?}",
        args
    );
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_IMAGES_LIST: IntegrationTest = IntegrationTest::new("images_list", test_images_list);

fn test_images_list() -> Result<()> {
    println!("Running test: bcvk images list --json");

    let sh = Shell::new()?;
    let bck = get_bck_command()?;

    // Run the bcvk images list command with JSON output
    let output = cmd!(sh, "{bck} images list --json").output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!("Failed to run 'bcvk images list --json': {}", stderr));
    }

    // Parse the JSON output
    let stdout = String::from_utf8(output.stdout)?;
    let images: Value = serde_json::from_str(&stdout).context("Failed to parse JSON output")?;

    // Verify the structure and content of the JSON
    let images_array = images
        .as_array()
        .ok_or_else(|| eyre!("Expected JSON array in output, got: {}", stdout))?;

    // Verify that the array contains valid image objects
    for (index, image) in images_array.iter().enumerate() {
        if !image.is_object() {
            return Err(eyre!(
                "Image entry {} is not a JSON object: {}",
                index,
                image
            ));
        }
    }

    println!(
        "Test passed: bck images list --json (found {} images)",
        images_array.len()
    );
    println!("All image entries are valid JSON objects");
    Ok(())
}

fn main() {
    let args = Arguments::from_args();

    // Collect tests from the distributed slice
    let tests: Vec<Trial> = INTEGRATION_TESTS
        .iter()
        .map(|test| {
            let name = test.name;
            let f = test.f;
            Trial::test(name, move || f().map_err(|e| format!("{:?}", e).into()))
        })
        .collect();

    // Run the tests and exit with the result
    libtest_mimic::run(&args, tests).exit();
}
