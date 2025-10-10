use camino::Utf8Path;
use std::process::Output;

use color_eyre::eyre::{eyre, Context};
use color_eyre::Result;
use libtest_mimic::{Arguments, Trial};
use serde_json::Value;
use xshell::{cmd, Shell};

// Re-export constants from lib for internal use
pub(crate) use integration_tests::{INTEGRATION_TEST_LABEL, LIBVIRT_INTEGRATION_TEST_LABEL};

mod tests {
    pub mod libvirt_base_disks;
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

/// Get libvirt connection arguments for CLI commands
///
/// Returns ["--connect", "URI"] if LIBVIRT_DEFAULT_URI is set, otherwise empty vec.
/// This uses the standard libvirt environment variable.
pub(crate) fn get_libvirt_connect_args() -> Vec<String> {
    if let Some(uri) = std::env::var("LIBVIRT_DEFAULT_URI").ok() {
        vec!["--connect".to_string(), uri]
    } else {
        vec![]
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

    let tests = vec![
        Trial::test("images_list", || {
            test_images_list()?;
            Ok(())
        }),
        Trial::test("run_ephemeral_correct_kernel", || {
            tests::run_ephemeral::test_run_ephemeral_correct_kernel();
            Ok(())
        }),
        Trial::test("run_ephemeral_poweroff", || {
            tests::run_ephemeral::test_run_ephemeral_poweroff();
            Ok(())
        }),
        Trial::test("run_ephemeral_with_memory_limit", || {
            tests::run_ephemeral::test_run_ephemeral_with_memory_limit();
            Ok(())
        }),
        Trial::test("run_ephemeral_with_vcpus", || {
            tests::run_ephemeral::test_run_ephemeral_with_vcpus();
            Ok(())
        }),
        Trial::test("run_ephemeral_execute", || {
            tests::run_ephemeral::test_run_ephemeral_execute();
            Ok(())
        }),
        Trial::test("run_ephemeral_container_ssh_access", || {
            tests::run_ephemeral::test_run_ephemeral_container_ssh_access();
            Ok(())
        }),
        Trial::test("run_ephemeral_ssh_command", || {
            tests::run_ephemeral_ssh::test_run_ephemeral_ssh_command();
            Ok(())
        }),
        Trial::test("run_ephemeral_ssh_cleanup", || {
            tests::run_ephemeral_ssh::test_run_ephemeral_ssh_cleanup();
            Ok(())
        }),
        Trial::test("run_ephemeral_ssh_system_command", || {
            tests::run_ephemeral_ssh::test_run_ephemeral_ssh_system_command();
            Ok(())
        }),
        Trial::test("run_ephemeral_ssh_exit_code", || {
            tests::run_ephemeral_ssh::test_run_ephemeral_ssh_exit_code();
            Ok(())
        }),
        Trial::test("run_ephemeral_ssh_cross_distro_compatibility", || {
            tests::run_ephemeral_ssh::test_run_ephemeral_ssh_cross_distro_compatibility();
            Ok(())
        }),
        Trial::test("mount_feature_bind", || {
            tests::mount_feature::test_mount_feature_bind();
            Ok(())
        }),
        Trial::test("mount_feature_ro_bind", || {
            tests::mount_feature::test_mount_feature_ro_bind();
            Ok(())
        }),
        Trial::test("to_disk", || {
            tests::to_disk::test_to_disk();
            Ok(())
        }),
        Trial::test("to_disk_qcow2", || {
            tests::to_disk::test_to_disk_qcow2();
            Ok(())
        }),
        Trial::test("to_disk_caching", || {
            tests::to_disk::test_to_disk_caching();
            Ok(())
        }),
        Trial::test("libvirt_list_functionality", || {
            tests::libvirt_verb::test_libvirt_list_functionality();
            Ok(())
        }),
        Trial::test("libvirt_list_json_output", || {
            tests::libvirt_verb::test_libvirt_list_json_output();
            Ok(())
        }),
        Trial::test("libvirt_run_resource_options", || {
            tests::libvirt_verb::test_libvirt_run_resource_options();
            Ok(())
        }),
        Trial::test("libvirt_run_networking", || {
            tests::libvirt_verb::test_libvirt_run_networking();
            Ok(())
        }),
        Trial::test("libvirt_ssh_integration", || {
            tests::libvirt_verb::test_libvirt_ssh_integration();
            Ok(())
        }),
        Trial::test("libvirt_run_ssh_full_workflow", || {
            tests::libvirt_verb::test_libvirt_run_ssh_full_workflow();
            Ok(())
        }),
        Trial::test("libvirt_vm_lifecycle", || {
            tests::libvirt_verb::test_libvirt_vm_lifecycle();
            Ok(())
        }),
        Trial::test("libvirt_label_functionality", || {
            tests::libvirt_verb::test_libvirt_label_functionality();
            Ok(())
        }),
        Trial::test("libvirt_error_handling", || {
            tests::libvirt_verb::test_libvirt_error_handling();
            Ok(())
        }),
        Trial::test("libvirt_bind_storage_ro", || {
            tests::libvirt_verb::test_libvirt_bind_storage_ro();
            Ok(())
        }),
        Trial::test("libvirt_base_disk_creation_and_reuse", || {
            tests::libvirt_base_disks::test_base_disk_creation_and_reuse();
            Ok(())
        }),
        Trial::test("libvirt_base_disks_list_command", || {
            tests::libvirt_base_disks::test_base_disks_list_command();
            Ok(())
        }),
        Trial::test("libvirt_base_disks_prune_dry_run", || {
            tests::libvirt_base_disks::test_base_disks_prune_dry_run();
            Ok(())
        }),
        Trial::test("libvirt_vm_disk_references_base", || {
            tests::libvirt_base_disks::test_vm_disk_references_base();
            Ok(())
        }),
    ];

    // Run the tests and exit with the result
    libtest_mimic::run(&args, tests).exit();
}
