//! Integration tests for libvirt to-base-disk functionality
//!
//! Tests the to-base-disk command which creates base disk images for libvirt VMs:
//! - Basic to-base-disk creation
//! - to-base-disk with different options
//! - to-base-disk reuse behavior
//! - Integration with libvirt base-disks list

use integration_tests::integration_test;
use itest::TestResult;
use xshell::cmd;

use regex::Regex;

use crate::{get_bck_command, get_test_image, shell};

/// Test basic to-base-disk command functionality
fn test_to_base_disk_basic() -> TestResult {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let test_image = get_test_image();

    println!("Testing basic to-base-disk functionality");

    // Run to-base-disk command
    let stdout = cmd!(sh, "{bck} libvirt to-base-disk {test_image}").read()?;

    println!("to-base-disk output: {}", stdout);

    // Should indicate successful creation
    assert!(
        stdout.contains("Created base disk:") || stdout.contains("Using cached"),
        "Should show creation or reuse of base disk, got: {}",
        stdout
    );

    // Extract disk path from output
    let disk_path_regex = Regex::new(r"Created base disk: (.+)").unwrap();
    let disk_path = if let Some(captures) = disk_path_regex.captures(&stdout) {
        captures.get(1).unwrap().as_str()
    } else {
        // If it's a cached disk, we need to check the list instead
        println!("Base disk was cached, checking list...");
        let list_output = cmd!(sh, "{bck} libvirt base-disks list").read()?;
        assert!(
            !list_output.contains("No base disk"),
            "Should have at least one base disk after creation"
        );
        return Ok(());
    };

    println!("Created disk path: {}", disk_path);

    // Verify the disk file exists
    assert!(
        std::path::Path::new(disk_path).exists(),
        "Created disk file should exist at: {}",
        disk_path
    );

    // Verify it shows up in base-disks list
    let list_output = cmd!(sh, "{bck} libvirt base-disks list").read()?;
    println!("base-disks list after creation:\n{}", list_output);

    // Should not be empty and should contain our disk
    assert!(
        !list_output.contains("No base disk"),
        "Should have base disks after creation"
    );

    println!("✓ Basic to-base-disk test passed");
    Ok(())
}
integration_test!(test_to_base_disk_basic);

/// Test to-base-disk with filesystem option
fn test_to_base_disk_with_filesystem() -> TestResult {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let test_image = get_test_image();

    println!("Testing to-base-disk with --filesystem option");

    // Run to-base-disk command with ext4 filesystem
    let stdout = cmd!(
        sh,
        "{bck} libvirt to-base-disk --filesystem ext4 {test_image}"
    )
    .read()?;

    println!("to-base-disk --filesystem ext4 output: {}", stdout);

    // Should indicate successful creation
    assert!(
        stdout.contains("Created base disk:") || stdout.contains("Using cached"),
        "Should show creation or reuse of base disk with filesystem option, got: {}",
        stdout
    );

    println!("✓ to-base-disk with filesystem option test passed");
    Ok(())
}
integration_test!(test_to_base_disk_with_filesystem);

/// Test to-base-disk reuse behavior
fn test_to_base_disk_reuse() -> TestResult {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let test_image = get_test_image();

    println!("Testing to-base-disk reuse behavior");

    // First call - might create or reuse existing
    let stdout1 = cmd!(sh, "{bck} libvirt to-base-disk {test_image}").read()?;
    println!("First to-base-disk call: {}", stdout1);

    // Second call with same image - should reuse
    let stdout2 = cmd!(sh, "{bck} libvirt to-base-disk {test_image}").read()?;
    println!("Second to-base-disk call: {}", stdout2);

    // At least one should show base disk creation/usage
    assert!(
        stdout1.contains("Created base disk:")
            || stdout1.contains("Using cached")
            || stdout2.contains("Created base disk:")
            || stdout2.contains("Using cached"),
        "Should show base disk creation or reuse"
    );

    println!("✓ to-base-disk reuse test passed");
    Ok(())
}
integration_test!(test_to_base_disk_reuse);

/// Test to-base-disk with root-size option
fn test_to_base_disk_with_root_size() -> TestResult {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let test_image = get_test_image();

    println!("Testing to-base-disk with --root-size option");

    // Run to-base-disk command with custom root size
    let stdout = cmd!(
        sh,
        "{bck} libvirt to-base-disk --root-size 15G {test_image}"
    )
    .read()?;

    println!("to-base-disk --root-size 15G output: {}", stdout);

    // Should indicate successful creation
    assert!(
        stdout.contains("Created base disk:") || stdout.contains("Using cached"),
        "Should show creation or reuse of base disk with root-size option, got: {}",
        stdout
    );

    println!("✓ to-base-disk with root-size option test passed");
    Ok(())
}
integration_test!(test_to_base_disk_with_root_size);

/// Test that to-base-disk integrates properly with base-disks commands
fn test_to_base_disk_integration_with_list() -> TestResult {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let test_image = get_test_image();

    println!("Testing to-base-disk integration with base-disks list");

    // Get initial count
    let initial_list = cmd!(sh, "{bck} libvirt base-disks list").read()?;
    let initial_count = if initial_list.contains("No base disk") {
        0
    } else {
        // Count lines with disk entries (skip header and summary)
        initial_list
            .lines()
            .filter(|line| {
                !line.contains("NAME") && !line.contains("Found") && !line.trim().is_empty()
            })
            .count()
    };

    println!("Initial base disk count: {}", initial_count);

    // Create base disk
    let stdout = cmd!(sh, "{bck} libvirt to-base-disk {test_image}").read()?;
    println!("to-base-disk output: {}", stdout);

    // Check final count
    let final_list = cmd!(sh, "{bck} libvirt base-disks list").read()?;
    println!("Final base-disks list:\n{}", final_list);

    if stdout.contains("Created base disk:") {
        // If we created a new disk, count should increase
        let final_count = final_list
            .lines()
            .filter(|line| {
                !line.contains("NAME") && !line.contains("Found") && !line.trim().is_empty()
            })
            .count();

        assert!(
            final_count > initial_count,
            "Base disk count should increase after creation"
        );
    } else {
        // If we reused existing, should still have disks listed
        assert!(
            !final_list.contains("No base disk"),
            "Should still have base disks listed after reuse"
        );
    }

    // Verify the list shows proper columns
    assert!(
        final_list.contains("NAME") && final_list.contains("SIZE"),
        "List should show proper table headers"
    );

    println!("✓ to-base-disk integration with list test passed");
    Ok(())
}
integration_test!(test_to_base_disk_integration_with_list);
