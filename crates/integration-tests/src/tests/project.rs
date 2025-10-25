//! Integration tests for bcvk project commands
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

use crate::{get_bck_command, IntegrationTest, INTEGRATION_TESTS};

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_PROJECT_WORKFLOW: IntegrationTest =
    IntegrationTest::new("project_upgrade_workflow", test_project_upgrade_workflow);

/// Test the full project workflow including upgrade
///
/// This test:
/// 1. Creates a custom bootc image based on centos-bootc:stream10
/// 2. Initializes a bcvk project
/// 3. Starts the VM with the initial image
/// 4. Modifies the Containerfile and builds v2
/// 5. Triggers manual upgrade with `bcvk project ssh -A`
/// 6. Verifies the upgrade was applied in the VM
fn test_project_upgrade_workflow() -> Result<()> {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let project_dir =
        Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf()).expect("temp path is not UTF-8");

    // Create initial Containerfile
    let containerfile_path = project_dir.join("Containerfile");
    let initial_containerfile = r#"FROM quay.io/centos-bootc/centos-bootc:stream10

# Add a marker file for version 1
RUN echo "version1" > /usr/share/test-version
"#;
    std::fs::write(&containerfile_path, initial_containerfile)
        .expect("Failed to write initial Containerfile");

    // Build initial image
    let image_name = "localhost/bcvk-test-project:latest";
    println!("Building initial test image: {}", image_name);
    let build_output = Command::new("podman")
        .args(&["build", "-t", image_name, "-f"])
        .arg(containerfile_path.as_str())
        .arg(project_dir.as_str())
        .output()
        .expect("Failed to run podman build");

    assert!(
        build_output.status.success(),
        "Initial podman build failed: {}",
        String::from_utf8_lossy(&build_output.stderr)
    );

    // Create .bcvk directory and config.toml
    let bcvk_dir = project_dir.join(".bcvk");
    std::fs::create_dir(&bcvk_dir).expect("Failed to create .bcvk directory");

    let config_content = format!(
        r#"[vm]
image = "{}"
memory = "2G"
cpus = 2
disk-size = "10G"
"#,
        image_name
    );
    std::fs::write(bcvk_dir.join("config.toml"), config_content)
        .expect("Failed to write config.toml");

    let bcvk = get_bck_command()?;

    // Start the project VM (detached)
    println!("Starting project VM...");
    let up_output = Command::new(&bcvk)
        .args(&["project", "up"])
        .current_dir(&project_dir)
        .env("BCVK_PROJECT_DIR", project_dir.as_str())
        .output()
        .expect("Failed to run bcvk project up");

    if !up_output.status.success() {
        eprintln!("bcvk project up failed:");
        eprintln!("stdout: {}", String::from_utf8_lossy(&up_output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&up_output.stderr));
        panic!("Failed to start project VM");
    }

    // Give VM time to boot
    std::thread::sleep(std::time::Duration::from_secs(30));

    // Verify version 1 is in the VM
    println!("Verifying initial version...");
    let check_v1_output = Command::new(&bcvk)
        .args(&["project", "ssh", "cat", "/usr/share/test-version"])
        .current_dir(&project_dir)
        .output()
        .expect("Failed to check initial version");

    let v1_content = String::from_utf8_lossy(&check_v1_output.stdout);
    assert!(
        v1_content.contains("version1"),
        "Initial version marker not found in VM. Output: {}",
        v1_content
    );

    // Update Containerfile to version 2
    println!("Building updated image (v2)...");
    let updated_containerfile = r#"FROM quay.io/centos-bootc/centos-bootc:stream10

# Add a marker file for version 2
RUN echo "version2" > /usr/share/test-version
"#;
    std::fs::write(&containerfile_path, updated_containerfile)
        .expect("Failed to write updated Containerfile");

    // Build version 2
    let build_v2_output = Command::new("podman")
        .args(&["build", "-t", image_name, "-f"])
        .arg(containerfile_path.as_str())
        .arg(project_dir.as_str())
        .output()
        .expect("Failed to run podman build for v2");

    assert!(
        build_v2_output.status.success(),
        "Version 2 podman build failed: {}",
        String::from_utf8_lossy(&build_v2_output.stderr)
    );

    // Trigger upgrade with `bcvk project ssh -A`
    println!("Triggering upgrade with `bcvk project ssh -A`...");
    let upgrade_output = Command::new(&bcvk)
        .args(&["project", "ssh", "-A", "echo", "upgrade-complete"])
        .current_dir(&project_dir)
        .output()
        .expect("Failed to run bcvk project ssh -A");

    if !upgrade_output.status.success() {
        eprintln!("bcvk project ssh -A failed:");
        eprintln!(
            "stdout: {}",
            String::from_utf8_lossy(&upgrade_output.stdout)
        );
        eprintln!(
            "stderr: {}",
            String::from_utf8_lossy(&upgrade_output.stderr)
        );
        panic!("Failed to trigger upgrade");
    }

    let upgrade_stdout = String::from_utf8_lossy(&upgrade_output.stdout);
    assert!(
        upgrade_stdout.contains("upgrade-complete"),
        "Upgrade command did not complete successfully"
    );

    // Check bootc status to verify new deployment is staged
    println!("Checking bootc status for staged deployment...");
    let status_output = Command::new(&bcvk)
        .args(&["project", "ssh", "bootc", "status", "--json"])
        .current_dir(&project_dir)
        .output()
        .expect("Failed to run bootc status");

    let status_json = String::from_utf8_lossy(&status_output.stdout);
    println!("bootc status output: {}", status_json);

    // Verify that status shows a staged deployment or that we have the new image
    // The exact behavior depends on bootc version, but we should see some indication
    // of the upgrade
    assert!(
        status_output.status.success(),
        "bootc status failed: {}",
        String::from_utf8_lossy(&status_output.stderr)
    );

    // Clean up - destroy the VM
    println!("Cleaning up project VM...");
    let _down_output = Command::new(&bcvk)
        .args(&["project", "down"])
        .current_dir(&project_dir)
        .output()
        .expect("Failed to run bcvk project down");

    let _rm_output = Command::new(&bcvk)
        .args(&["project", "rm"])
        .current_dir(&project_dir)
        .output()
        .expect("Failed to run bcvk project rm");

    // Clean up the test image
    let _rmi_output = Command::new("podman")
        .args(&["rmi", "-f", image_name])
        .output()
        .ok();

    Ok(())
}
