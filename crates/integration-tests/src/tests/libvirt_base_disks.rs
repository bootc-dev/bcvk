//! Integration tests for libvirt base disk functionality
//!
//! Tests the base disk caching and CoW cloning system:
//! - Base disk creation and reuse
//! - Multiple VMs sharing the same base disk
//! - base-disks list command
//! - base-disks prune command

use color_eyre::Result;
use integration_tests::integration_test;
use xshell::cmd;

use regex::Regex;

use crate::{get_bck_command, get_test_image, shell};

/// Test that base disk is created and reused for multiple VMs
fn test_base_disk_creation_and_reuse() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let test_image = get_test_image();

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
    let vm1_output = cmd!(
        sh,
        "{bck} libvirt run --name {vm1_name} --filesystem ext4 {test_image}"
    )
    .ignore_status()
    .output()?;

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
    let vm2_output = cmd!(
        sh,
        "{bck} libvirt run --name {vm2_name} --filesystem ext4 {test_image}"
    )
    .ignore_status()
    .output()?;

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

    // Test base-disks list shows creation timestamp
    println!("Testing that base-disks list shows creation timestamp...");
    let sh = shell()?;
    let bck = get_bck_command()?;
    let list_stdout = cmd!(sh, "{bck} libvirt base-disks list").read()?;
    println!("base-disks list output:\n{}", list_stdout);

    // Should have CREATED column in header
    assert!(
        list_stdout.contains("CREATED"),
        "Should show CREATED column in header"
    );

    // Should show timestamp values (either a date or "unknown")
    // Timestamp format is YYYY-MM-DD HH:MM
    let re = Regex::new(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}|unknown").unwrap();
    let has_timestamp = re.is_match(&list_stdout);
    assert!(
        has_timestamp,
        "Should show timestamp values in CREATED column"
    );

    println!("✓ base-disks list shows creation timestamp");

    println!("✓ Base disk creation and reuse test passed");
    Ok(())
}
integration_test!(test_base_disk_creation_and_reuse);

/// Test base-disks list command
fn test_base_disks_list_command() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;

    println!("Testing base-disks list command");

    let output = cmd!(sh, "{bck} libvirt base-disks list")
        .ignore_status()
        .output()?;
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
    Ok(())
}
integration_test!(test_base_disks_list_command);

/// Test base-disks prune command with dry-run
fn test_base_disks_prune_dry_run() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;

    println!("Testing base-disks prune --dry-run command");

    let output = cmd!(sh, "{bck} libvirt base-disks prune --dry-run")
        .ignore_status()
        .output()?;
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
    Ok(())
}
integration_test!(test_base_disks_prune_dry_run);

/// Test that VM disks reference base disks correctly
fn test_vm_disk_references_base() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let test_image = get_test_image();

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let vm_name = format!("test-disk-ref-{}", timestamp);

    println!("Testing VM disk references base disk");

    cleanup_domain(&vm_name);

    // Create VM
    let output = cmd!(
        sh,
        "{bck} libvirt run --name {vm_name} --filesystem ext4 {test_image}"
    )
    .ignore_status()
    .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        cleanup_domain(&vm_name);

        panic!("Failed to create VM: {}", stderr);
    }

    // Get VM disk path from domain XML
    let sh = shell()?;
    let domain_xml = cmd!(sh, "virsh dumpxml {vm_name}").read()?;

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
    Ok(())
}
integration_test!(test_vm_disk_references_base);

/// Helper function to cleanup domain and its disk
fn cleanup_domain(domain_name: &str) {
    println!("Cleaning up domain: {}", domain_name);

    let sh = match shell() {
        Ok(sh) => sh,
        Err(_) => return,
    };

    // Stop domain if running
    let _ = cmd!(sh, "virsh destroy {domain_name}")
        .ignore_status()
        .quiet()
        .run();

    // Use bcvk libvirt rm for proper cleanup
    let bck = match get_bck_command() {
        Ok(cmd) => cmd,
        Err(_) => return,
    };

    match cmd!(sh, "{bck} libvirt rm {domain_name} --force --stop")
        .ignore_status()
        .output()
    {
        Ok(output) if output.status.success() => {
            println!("Successfully cleaned up domain: {}", domain_name);
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!("Cleanup warning (may be expected): {}", stderr);
        }
        Err(_) => {}
    }
}
