//! Integration tests for bollard-based podman API
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

use color_eyre::Result;
use integration_tests::integration_test;

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{get_bck_command, INTEGRATION_TEST_LABEL};

fn test_bollard_container_removal() -> Result<()> {
    // Generate unique container name
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs();
    let container_name = format!("bcvk-bollard-test-{}", timestamp);

    // Create a test container with the integration test label
    let create_output = Command::new("podman")
        .args([
            "run",
            "-d",
            "--name",
            &container_name,
            "--label",
            INTEGRATION_TEST_LABEL,
            "docker.io/library/alpine:latest",
            "sleep",
            "300",
        ])
        .output()
        .expect("Failed to create test container");

    assert!(
        create_output.status.success(),
        "Failed to create test container: {}",
        String::from_utf8_lossy(&create_output.stderr)
    );

    // Verify container was created
    let verify_output = Command::new("podman")
        .args(["ps", "-a", "--filter", &format!("name={}", container_name)])
        .output()
        .expect("Failed to verify container creation");

    assert!(
        verify_output.status.success(),
        "Failed to verify container creation: {}",
        String::from_utf8_lossy(&verify_output.stderr)
    );

    let ps_output = String::from_utf8_lossy(&verify_output.stdout);
    assert!(
        ps_output.contains(&container_name),
        "Container {} not found in podman ps output",
        container_name
    );

    // Use bollard to remove the container via the CLI
    let bcvk_path = get_bck_command().expect("Failed to get bcvk command");
    let remove_output = Command::new(&bcvk_path)
        .args([
            "internals",
            "bollard",
            "remove-container",
            "--force",
            &container_name,
        ])
        .output()
        .expect("Failed to run bcvk internals bollard remove-container");

    assert!(
        remove_output.status.success(),
        "Failed to remove container via bollard: {}",
        String::from_utf8_lossy(&remove_output.stderr)
    );

    // Verify container was removed
    let verify_removal = Command::new("podman")
        .args(["ps", "-a", "--filter", &format!("name={}", container_name)])
        .output()
        .expect("Failed to verify container removal");

    assert!(
        verify_removal.status.success(),
        "Failed to verify container removal: {}",
        String::from_utf8_lossy(&verify_removal.stderr)
    );

    let removal_output = String::from_utf8_lossy(&verify_removal.stdout);
    assert!(
        !removal_output.contains(&container_name),
        "Container {} still exists after removal",
        container_name
    );

    Ok(())
}
integration_test!(test_bollard_container_removal);
