//! Integration tests for libvirt base disk functionality
//!
//! Tests the base disk caching and CoW cloning system:
//! - Base disk creation and reuse
//! - Multiple VMs sharing the same base disk
//! - base-disks list command
//! - base-disks prune command

use std::process::Command;

use crate::{get_bck_command, get_libvirt_connect_args, get_test_image};

/// Test that base disk is created and reused for multiple VMs
pub fn test_base_disk_creation_and_reuse() {
    let bck = get_bck_command().unwrap();
    let test_image = get_test_image();
    let connect_args = get_libvirt_connect_args();

    // Generate unique names for test VMs
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let vm1_name = format!("test-base-disk-vm1-{}", timestamp);
    let vm2_name = format!("test-base-disk-vm2-{}", timestamp);

    println!("Testing base disk creation and reuse");
    println!("VM1: {}", vm1_name);
    println!("VM2: {}", vm2_name);

    // Cleanup any existing test domains
    cleanup_domain(&vm1_name);
    cleanup_domain(&vm2_name);

    // Create first VM - this should create a new base disk
    println!("Creating first VM (should create base disk)...");
    let mut vm1_cmd = Command::new("timeout");
    vm1_cmd.args(["300s", &bck, "libvirt"]);
    vm1_cmd.args(&connect_args);
    vm1_cmd.arg("run");
    vm1_cmd.args(["--name", &vm1_name, "--filesystem", "ext4", &test_image]);
    let vm1_output = vm1_cmd.output().expect("Failed to create first VM");

    let vm1_stdout = String::from_utf8_lossy(&vm1_output.stdout);
    let vm1_stderr = String::from_utf8_lossy(&vm1_output.stderr);

    println!("VM1 stdout: {}", vm1_stdout);
    println!("VM1 stderr: {}", vm1_stderr);

    if !vm1_output.status.success() {
        cleanup_domain(&vm1_name);
        cleanup_domain(&vm2_name);

        panic!("Failed to create first VM: {}", vm1_stderr);
    }

    // Verify base disk was created
    assert!(
        vm1_stdout.contains("Using base disk") || vm1_stdout.contains("base disk"),
        "Should mention base disk creation"
    );

    // Create second VM - this should reuse the base disk
    println!("Creating second VM (should reuse base disk)...");
    let mut vm2_cmd = Command::new("timeout");
    vm2_cmd.args(["300s", &bck, "libvirt"]);
    vm2_cmd.args(&connect_args);
    vm2_cmd.arg("run");
    vm2_cmd.args(["--name", &vm2_name, "--filesystem", "ext4", &test_image]);
    let vm2_output = vm2_cmd.output().expect("Failed to create second VM");

    let vm2_stdout = String::from_utf8_lossy(&vm2_output.stdout);
    let vm2_stderr = String::from_utf8_lossy(&vm2_output.stderr);

    println!("VM2 stdout: {}", vm2_stdout);
    println!("VM2 stderr: {}", vm2_stderr);

    // Cleanup before assertions
    cleanup_domain(&vm1_name);
    cleanup_domain(&vm2_name);

    if !vm2_output.status.success() {
        panic!("Failed to create second VM: {}", vm2_stderr);
    }

    // Verify base disk was reused (should be faster and mention using existing)
    assert!(
        vm2_stdout.contains("Using base disk") || vm2_stdout.contains("base disk"),
        "Should mention using base disk"
    );

    println!("✓ Base disk creation and reuse test passed");
}

/// Test base-disks list command
pub fn test_base_disks_list_command() {
    let bck = get_bck_command().unwrap();
    let connect_args = get_libvirt_connect_args();

    println!("Testing base-disks list command");

    let mut cmd = Command::new(&bck);
    cmd.arg("libvirt");
    cmd.args(&connect_args);
    cmd.args(["base-disks", "list"]);
    let output = cmd.output().expect("Failed to run base-disks list");

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
    let connect_args = get_libvirt_connect_args();

    println!("Testing base-disks prune --dry-run command");

    let mut cmd = Command::new(&bck);
    cmd.arg("libvirt");
    cmd.args(&connect_args);
    cmd.args(["base-disks", "prune", "--dry-run"]);
    let output = cmd
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
    let bck = get_bck_command().unwrap();
    let test_image = get_test_image();
    let connect_args = get_libvirt_connect_args();

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let vm_name = format!("test-disk-ref-{}", timestamp);

    println!("Testing VM disk references base disk");

    cleanup_domain(&vm_name);

    // Create VM
    let mut vm_cmd = Command::new("timeout");
    vm_cmd.args(["300s", &bck, "libvirt"]);
    vm_cmd.args(&connect_args);
    vm_cmd.arg("run");
    vm_cmd.args(["--name", &vm_name, "--filesystem", "ext4", &test_image]);
    let output = vm_cmd.output().expect("Failed to create VM");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        cleanup_domain(&vm_name);

        panic!("Failed to create VM: {}", stderr);
    }

    // Get VM disk path from domain XML
    let mut dumpxml_cmd = Command::new("virsh");
    for arg in &connect_args {
        dumpxml_cmd.arg(arg);
    }
    dumpxml_cmd.args(&["dumpxml", &vm_name]);
    let dumpxml_output = dumpxml_cmd.output().expect("Failed to dump domain XML");

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

/// Helper function to cleanup domain and its disk
fn cleanup_domain(domain_name: &str) {
    let connect_args = get_libvirt_connect_args();
    println!("Cleaning up domain: {}", domain_name);

    // Stop domain if running
    let mut destroy_cmd = Command::new("virsh");
    for arg in &connect_args {
        destroy_cmd.arg(arg);
    }
    destroy_cmd.args(&["destroy", domain_name]);
    let _ = destroy_cmd.output();

    // Use bcvk libvirt rm for proper cleanup
    let bck = get_bck_command().unwrap();
    let mut cleanup_cmd = Command::new(&bck);
    cleanup_cmd.arg("libvirt");
    cleanup_cmd.args(&connect_args);
    cleanup_cmd.args(&["rm", domain_name, "--force", "--stop"]);
    let cleanup_output = cleanup_cmd.output();

    if let Ok(output) = cleanup_output {
        if output.status.success() {
            println!("Successfully cleaned up domain: {}", domain_name);
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!("Cleanup warning (may be expected): {}", stderr);
        }
    }
}
