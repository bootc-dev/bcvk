//! Integration tests for ephemeral test-basic command
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

use integration_tests::{integration_test, parameterized_integration_test};
use itest::TestResult;
use xshell::cmd;

use crate::{get_bck_command, get_test_image, shell};

/// Test the basic smoke test command
///
/// This test verifies that `bcvk ephemeral test-basic` successfully boots
/// a bootc container image and verifies systemd reaches a healthy state.
fn test_ephemeral_test_basic() -> TestResult {
    println!("Running test: bcvk ephemeral test-basic");

    let sh = shell()?;
    let bcvk = get_bck_command()?;
    let image = get_test_image();

    println!("Testing with image: {}", image);

    // Run the test-basic command
    // This should boot the VM, check systemd health, and clean up
    let output = cmd!(sh, "{bcvk} ephemeral test-basic {image}").output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        eprintln!("Command failed with exit code: {:?}", output.status.code());
        eprintln!("stdout: {}", stdout);
        eprintln!("stderr: {}", stderr);
        return Err(anyhow::anyhow!(
            "test-basic command failed for image {}",
            image
        )
        .into());
    }

    // Verify expected output
    assert!(
        stdout.contains("System health check passed"),
        "Expected success message not found in output. stdout: {}",
        stdout
    );

    println!("Test passed: bcvk ephemeral test-basic");
    Ok(())
}
integration_test!(test_ephemeral_test_basic);

/// Parameterized test that runs test-basic against all configured test images
///
/// Uses BCVK_ALL_IMAGES environment variable to get the list of images to test.
fn test_ephemeral_test_basic_parameterized(image: &str) -> TestResult {
    println!("Running parameterized test: bcvk ephemeral test-basic (image: {})", image);

    let sh = shell()?;
    let bcvk = get_bck_command()?;

    // Run the test-basic command
    let output = cmd!(sh, "{bcvk} ephemeral test-basic {image}").output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        eprintln!("Command failed with exit code: {:?}", output.status.code());
        eprintln!("stdout: {}", stdout);
        eprintln!("stderr: {}", stderr);
        return Err(anyhow::anyhow!(
            "test-basic command failed for image {}",
            image
        )
        .into());
    }

    // Verify expected output
    assert!(
        stdout.contains("System health check passed"),
        "Expected success message not found in output for image {}. stdout: {}",
        image,
        stdout
    );

    println!("Parameterized test passed for image: {}", image);
    Ok(())
}
parameterized_integration_test!(test_ephemeral_test_basic_parameterized);
