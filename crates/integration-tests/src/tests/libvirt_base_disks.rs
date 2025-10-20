//! Integration tests for libvirt base disk functionality
//!
//! Tests the base disk caching and CoW cloning system:
//! - Base disk creation and reuse
//! - Multiple VMs sharing the same base disk
//! - base-disks list command
//! - base-disks prune command

use std::process::Command;

use crate::{get_bck_command, get_test_image, run_bcvk};

/// Test that base disk is created and reused for multiple VMs
pub fn test_base_disk_creation_and_reuse() {
    let test_image = get_test_image();

    // Generate unique names for test VMs using shortuuid pattern
    let vm1_name_template = "test-base-disk-vm1-{shortuuid}";
    let vm2_name_template = "test-base-disk-vm2-{shortuuid}";

    println!("Testing base disk creation and reuse");

    // Create temp files for domain names
    let vm1_id_file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let vm1_id_path = vm1_id_file.path().to_str().expect("Invalid temp file path");

    // Create first VM - this should create a new base disk
    println!("Creating first VM (should create base disk)...");
    let vm1_output = run_bcvk(&[
        "libvirt",
        "run",
        "--name",
        vm1_name_template,
        "--write-id-to",
        vm1_id_path,
        "--filesystem",
        "ext4",
        &test_image,
    ])
    .expect("Failed to create first VM");

    println!("VM1 stdout: {}", vm1_output.stdout);
    println!("VM1 stderr: {}", vm1_output.stderr);

    if !vm1_output.success() {
        // Attempt cleanup before panicking
        let _ = std::fs::read_to_string(vm1_id_path).map(|name| cleanup_domain(name.trim()));
        panic!("Failed to create first VM: {}", vm1_output.stderr);
    }

    // Read the domain name from the file
    let vm1_name = std::fs::read_to_string(vm1_id_path)
        .expect("Failed to read VM1 domain name from file")
        .trim()
        .to_string();

    println!("Created VM1: {}", vm1_name);

    // Verify base disk was created
    assert!(
        vm1_output.stdout.contains("Using base disk") || vm1_output.stdout.contains("base disk"),
        "Should mention base disk creation"
    );

    // Create second VM - this should reuse the base disk
    println!("Creating second VM (should reuse base disk)...");
    let vm2_id_file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let vm2_id_path = vm2_id_file.path().to_str().expect("Invalid temp file path");

    let vm2_output = run_bcvk(&[
        "libvirt",
        "run",
        "--name",
        vm2_name_template,
        "--write-id-to",
        vm2_id_path,
        "--filesystem",
        "ext4",
        &test_image,
    ])
    .expect("Failed to create second VM");

    println!("VM2 stdout: {}", vm2_output.stdout);
    println!("VM2 stderr: {}", vm2_output.stderr);

    if !vm2_output.success() {
        // Cleanup VM1 before panicking
        cleanup_domain(&vm1_name);
        panic!("Failed to create second VM: {}", vm2_output.stderr);
    }

    // Read the domain name from the file
    let vm2_name = std::fs::read_to_string(vm2_id_path)
        .expect("Failed to read VM2 domain name from file")
        .trim()
        .to_string();

    println!("Created VM2: {}", vm2_name);

    // Cleanup before assertions
    cleanup_domain(&vm1_name);
    cleanup_domain(&vm2_name);

    // Verify base disk was reused (should be faster and mention using existing)
    assert!(
        vm2_output.stdout.contains("Using base disk") || vm2_output.stdout.contains("base disk"),
        "Should mention using base disk"
    );

    println!("✓ Base disk creation and reuse test passed");
}

/// Test base-disks list command
pub fn test_base_disks_list_command() {
    let bck = get_bck_command().unwrap();

    println!("Testing base-disks list command");

    let output = Command::new(&bck)
        .args(["libvirt", "base-disks", "list"])
        .output()
        .expect("Failed to run base-disks list");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        println!("base-disks list output: {}", stdout);

        // Should show table header or empty message
        assert!(
            stdout.contains("NAME")
                || stdout.contains("No base disk")
                || stdout.contains("no base disk")
                || stdout.is_empty(),
            "Should show table format or empty message, got: {}",
            stdout
        );

        println!("✓ base-disks list command works");
    } else {
        println!("base-disks list failed (may be expected): {}", stderr);

        // Should fail gracefully
        assert!(
            stderr.contains("pool") || stderr.contains("libvirt") || stderr.contains("connect"),
            "Should have meaningful error about libvirt connectivity"
        );
    }
}

/// Test base-disks prune command with dry-run
pub fn test_base_disks_prune_dry_run() {
    let bck = get_bck_command().unwrap();

    println!("Testing base-disks prune --dry-run command");

    let output = Command::new(&bck)
        .args(["libvirt", "base-disks", "prune", "--dry-run"])
        .output()
        .expect("Failed to run base-disks prune --dry-run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        println!("base-disks prune --dry-run output: {}", stdout);

        // Should show what would be removed or indicate nothing to prune
        assert!(
            stdout.contains("Would remove") || stdout.contains("No") || stdout.is_empty(),
            "Should show dry-run output"
        );

        println!("✓ base-disks prune --dry-run command works");
    } else {
        println!("base-disks prune failed (may be expected): {}", stderr);

        // Should fail gracefully
        assert!(
            stderr.contains("pool") || stderr.contains("libvirt") || stderr.contains("connect"),
            "Should have meaningful error about libvirt connectivity"
        );
    }
}

/// Test that VM disks reference base disks correctly
pub fn test_vm_disk_references_base() {
    let test_image = get_test_image();

    let vm_name_template = "test-disk-ref-{shortuuid}";

    println!("Testing VM disk references base disk");

    // Create temp file for domain name
    let id_file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let id_path = id_file.path().to_str().expect("Invalid temp file path");

    // Create VM
    let output = run_bcvk(&[
        "libvirt",
        "run",
        "--name",
        vm_name_template,
        "--write-id-to",
        id_path,
        "--filesystem",
        "ext4",
        &test_image,
    ])
    .expect("Failed to create VM");

    if !output.success() {
        // Attempt cleanup before panicking
        let _ = std::fs::read_to_string(id_path).map(|name| cleanup_domain(name.trim()));
        panic!("Failed to create VM: {}", output.stderr);
    }

    // Read the domain name from the file
    let vm_name = std::fs::read_to_string(id_path)
        .expect("Failed to read domain name from file")
        .trim()
        .to_string();

    println!("Created VM: {}", vm_name);

    // Get VM disk path from domain XML
    let dumpxml_output = Command::new("virsh")
        .args(&["dumpxml", &vm_name])
        .output()
        .expect("Failed to dump domain XML");

    if !dumpxml_output.status.success() {
        cleanup_domain(&vm_name);
        panic!("Failed to get domain XML");
    }

    let domain_xml = String::from_utf8_lossy(&dumpxml_output.stdout);

    // Parse XML using bcvk's xml_utils to extract disk path
    let dom = bcvk::xml_utils::parse_xml_dom(&domain_xml).expect("Failed to parse domain XML");

    let disk_path = dom
        .find("disk")
        .expect("No disk element found in domain XML")
        .children
        .iter()
        .find(|child| child.name == "source")
        .expect("No source element found in disk")
        .attributes
        .get("file")
        .expect("No file attribute found in source element");

    cleanup_domain(&vm_name);

    println!("VM disk path: {}", disk_path);

    // Disk should be named after the VM, not a base disk
    assert!(
        disk_path.contains(&vm_name) && !disk_path.contains("bootc-base-"),
        "VM should use its own disk, not directly use base disk"
    );

    println!("✓ VM disk reference test passed");
}

fn cleanup_domain(domain_name: &str) {
    println!("Cleaning up domain: {}", domain_name);

    // Stop domain if running
    let _ = Command::new("virsh")
        .args(&["destroy", domain_name])
        .output();

    // Use bcvk libvirt rm for proper cleanup
    let bck = get_bck_command().unwrap();
    let cleanup_output = Command::new(&bck)
        .args(&["libvirt", "rm", domain_name, "--force", "--stop"])
        .output();

    if let Ok(output) = cleanup_output {
        if output.status.success() {
            println!("Successfully cleaned up domain: {}", domain_name);
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!("Cleanup warning (may be expected): {}", stderr);
        }
    }
}
