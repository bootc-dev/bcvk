//! Cleanup utility for integration test resources
//!
//! This binary removes integration test containers and libvirt VMs that were created during testing.

use std::process::Command;

// Import shared constants from the library
use integration_tests::{INTEGRATION_TEST_LABEL, LIBVIRT_INTEGRATION_TEST_LABEL};

fn cleanup_integration_test_containers() -> Result<(), Box<dyn std::error::Error>> {
    println!("Cleaning up integration test containers...");

    // List all containers with our integration test label
    let list_output = Command::new("podman")
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("label={}", INTEGRATION_TEST_LABEL),
            "-q",
        ])
        .output()?;

    if !list_output.status.success() {
        eprintln!("Warning: Failed to list containers");
        return Ok(());
    }

    let container_ids = String::from_utf8_lossy(&list_output.stdout);
    let containers: Vec<&str> = container_ids.lines().filter(|l| !l.is_empty()).collect();

    if containers.is_empty() {
        println!("No integration test containers found to clean up");
        return Ok(());
    }

    println!(
        "Found {} integration test container(s) to clean up",
        containers.len()
    );

    // Force remove each container
    let mut cleaned = 0;
    for container_id in containers {
        print!(
            "  Removing container {}... ",
            &container_id[..12.min(container_id.len())]
        );
        let rm_output = Command::new("podman")
            .args(["rm", "-f", "--", container_id])
            .output()?;

        if rm_output.status.success() {
            println!("✓");
            cleaned += 1;
        } else {
            println!("✗ (failed)");
            eprintln!("    Error: {}", String::from_utf8_lossy(&rm_output.stderr));
        }
    }

    println!("Cleanup completed: {} container(s) removed", cleaned);
    Ok(())
}

fn cleanup_libvirt_integration_test_vms() -> Result<(), Box<dyn std::error::Error>> {
    println!("Cleaning up integration test libvirt VMs...");

    // Get path to bcvk binary (should be in the same directory as this cleanup binary)
    let current_exe = std::env::current_exe()?;
    let bcvk_path = current_exe
        .parent()
        .ok_or("Failed to get parent directory")?
        .join("bcvk");

    if !bcvk_path.exists() {
        println!(
            "bcvk binary not found at {:?}, skipping libvirt cleanup",
            bcvk_path
        );
        return Ok(());
    }

    // Use bcvk libvirt rm-all with label filter
    let rm_output = Command::new(&bcvk_path)
        .args([
            "libvirt",
            "rm-all",
            "--label",
            LIBVIRT_INTEGRATION_TEST_LABEL,
            "--force",
            "--stop",
        ])
        .output()?;

    if !rm_output.status.success() {
        let stderr = String::from_utf8_lossy(&rm_output.stderr);
        eprintln!("Warning: Failed to clean up libvirt VMs: {}", stderr);
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&rm_output.stdout);
    println!("{}", stdout);

    Ok(())
}

fn main() {
    let mut errors = Vec::new();

    if let Err(e) = cleanup_integration_test_containers() {
        eprintln!("Error during container cleanup: {}", e);
        errors.push(format!("containers: {}", e));
    }

    if let Err(e) = cleanup_libvirt_integration_test_vms() {
        eprintln!("Error during libvirt VM cleanup: {}", e);
        errors.push(format!("libvirt: {}", e));
    }

    if !errors.is_empty() {
        eprintln!("Cleanup completed with errors: {}", errors.join(", "));
        std::process::exit(1);
    }
}
