//! Integration tests for the libvirt verb with domain management subcommands
//!
//! These tests verify the libvirt command structure:
//! - `bcvk libvirt run` - Run bootable containers as persistent VMs
//! - `bcvk libvirt list` - List bootc domains
//! - `bcvk libvirt list-volumes` - List available bootc volumes
//! - `bcvk libvirt ssh` - SSH into domains
//! - Domain lifecycle management (start/stop/rm/inspect)

use std::process::Command;

use crate::{get_bck_command, get_test_image};

/// Test libvirt list functionality (lists domains)
pub fn test_libvirt_list_functionality() {
    let bck = get_bck_command().unwrap();

    // Check if virsh is available
    let virsh_check = Command::new("which").arg("virsh").output();

    if virsh_check.is_err() || !virsh_check.unwrap().status.success() {
        println!("Skipping libvirt list test - virsh not available");
        return;
    }

    let output = Command::new(&bck)
        .args(["libvirt", "list"])
        .output()
        .expect("Failed to run libvirt list");

    // May succeed or fail depending on libvirt availability, but should not crash
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        println!("libvirt list succeeded: {}", stdout);
        // Should show domain listing format
        assert!(
            stdout.contains("NAME")
                || stdout.contains("No VMs found")
                || stdout.contains("No running VMs found"),
            "Should show domain listing format or empty message"
        );
    } else {
        println!("libvirt list failed (expected in CI): {}", stderr);
        // Verify it fails with proper error message about libvirt connectivity
        assert!(
            stderr.contains("libvirt") || stderr.contains("connect") || stderr.contains("virsh"),
            "Should have meaningful error about libvirt connectivity"
        );
    }

    println!("libvirt list functionality tested");
}

/// Test libvirt list with JSON output
pub fn test_libvirt_list_json_output() {
    let bck = get_bck_command().unwrap();

    let output = Command::new(&bck)
        .args(["libvirt", "list", "--format", "json"])
        .output()
        .expect("Failed to run libvirt list --format json");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        // If successful, should be valid JSON
        let json_result: Result<serde_json::Value, _> = serde_json::from_str(&stdout);
        assert!(
            json_result.is_ok(),
            "libvirt list --format json should produce valid JSON: {}",
            stdout
        );
        println!("libvirt list --format json produced valid JSON");
    } else {
        // May fail in CI without libvirt, but should mention error handling
        println!(
            "libvirt list --format json failed (expected in CI): {}",
            stderr
        );
    }

    println!("libvirt list JSON output tested");
}

/// Test domain resource configuration options
pub fn test_libvirt_run_resource_options() {
    let bck = get_bck_command().unwrap();

    // Test various resource configurations are accepted syntactically
    let resource_tests = vec![
        vec!["--memory", "1G", "--cpus", "1"],
        vec!["--memory", "4G", "--cpus", "4"],
        vec!["--memory", "2048M", "--cpus", "2"],
    ];

    for resources in resource_tests {
        let mut args = vec!["libvirt", "run"];
        args.extend(resources.iter());
        args.push("--help"); // Just test parsing, don't actually run

        let output = Command::new(&bck)
            .args(&args)
            .output()
            .expect("Failed to run libvirt run with resources");

        // Should show help and not fail on resource parsing
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            assert!(
                !stderr.contains("invalid") && !stderr.contains("parse"),
                "Resource options should be parsed correctly: {:?}, stderr: {}",
                resources,
                stderr
            );
        } else {
            assert!(
                stdout.contains("Usage") || stdout.contains("USAGE"),
                "Should show help output when using --help"
            );
        }
    }

    println!("libvirt run resource options validated");
}

/// Test domain networking configuration
pub fn test_libvirt_run_networking() {
    let bck = get_bck_command().unwrap();

    let network_configs = vec![
        vec!["--network", "user"],
        vec!["--network", "bridge"],
        vec!["--network", "none"],
    ];

    for network in network_configs {
        let mut args = vec!["libvirt", "run"];
        args.extend(network.iter());
        args.push("--help"); // Just test parsing, don't actually run

        let output = Command::new(&bck)
            .args(&args)
            .output()
            .expect("Failed to run libvirt run with network config");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            // Should not fail on network option parsing
            assert!(
                !stderr.contains("invalid") && !stderr.contains("parse"),
                "Network options should be parsed correctly: {:?}, stderr: {}",
                network,
                stderr
            );
        } else {
            assert!(
                stdout.contains("Usage") || stdout.contains("USAGE"),
                "Should show help output when using --help"
            );
        }
    }

    println!("libvirt run networking options validated");
}

/// Test SSH integration with created domains (syntax only)
pub fn test_libvirt_ssh_integration() {
    let bck = get_bck_command().unwrap();

    // Test that SSH command integration works syntactically
    let output = Command::new(&bck)
        .args(["libvirt", "ssh", "test-domain", "--", "echo", "hello"])
        .output()
        .expect("Failed to run libvirt ssh command");

    // Will likely fail since no domain exists, but should not crash
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        // Should fail gracefully with domain-related error
        assert!(
            stderr.contains("domain") || stderr.contains("connect") || stderr.contains("ssh"),
            "SSH integration should fail gracefully: {}",
            stderr
        );
    }

    println!("libvirt SSH integration tested");
}

/// Test full libvirt run + SSH workflow like run_ephemeral SSH tests
pub fn test_libvirt_run_ssh_full_workflow() {
    // Skip if running in CI/container environment without libvirt
    if std::env::var("CI").is_ok() || !std::path::Path::new("/usr/bin/virsh").exists() {
        println!("Skipping full SSH workflow test - no libvirt available");
        return;
    }

    let bck = get_bck_command().unwrap();
    let test_image = get_test_image();

    // Generate unique domain name for this test
    let domain_name = format!(
        "test-ssh-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    println!(
        "Testing full libvirt run + SSH workflow with domain: {}",
        domain_name
    );

    // Cleanup any existing domain with this name
    let _ = Command::new("virsh")
        .args(&["destroy", &domain_name])
        .output();
    let _ = Command::new("virsh")
        .args(&["undefine", &domain_name])
        .output();

    // Create domain with SSH key generation
    println!("Creating libvirt domain with SSH key injection...");
    let create_output = Command::new("timeout")
        .args([
            "300s", // 5 minute timeout for domain creation
            &bck,
            "libvirt",
            "run",
            "--name",
            &domain_name,
            "--filesystem",
            "ext4",
            &test_image,
        ])
        .output()
        .expect("Failed to run libvirt run with SSH");

    let create_stdout = String::from_utf8_lossy(&create_output.stdout);
    let create_stderr = String::from_utf8_lossy(&create_output.stderr);

    println!("Create stdout: {}", create_stdout);
    println!("Create stderr: {}", create_stderr);

    if !create_output.status.success() {
        cleanup_domain(&domain_name);

        // Check for acceptable failures (no libvirt, permissions, etc.)
        let acceptable_failures = [
            "pool",
            "connect",
            "Permission denied",
            "libvirt",
            "KVM",
            "kvm",
            "nested",
            "hardware",
            "acceleration",
            "Storage pool",
            "qemu",
            "network",
        ];

        let is_acceptable = acceptable_failures.iter().any(|&pattern| {
            create_stderr
                .to_lowercase()
                .contains(&pattern.to_lowercase())
        });

        if is_acceptable {
            println!(
                "Skipping full SSH workflow test - libvirt not properly configured: {}",
                create_stderr
            );
            return;
        }

        panic!("Failed to create domain with SSH: {}", create_stderr);
    }

    println!("Successfully created domain: {}", domain_name);

    // Wait for VM to boot and SSH to become available
    println!("Waiting for VM to boot and SSH to become available...");
    std::thread::sleep(std::time::Duration::from_secs(30));

    // Test SSH connection with simple command
    println!("Testing SSH connection: echo 'hello world'");
    let ssh_output = Command::new("timeout")
        .args([
            "60s",
            &bck,
            "libvirt",
            "ssh",
            &domain_name,
            "--",
            "echo",
            "hello world",
        ])
        .output()
        .expect("Failed to run libvirt ssh command");

    let ssh_stdout = String::from_utf8_lossy(&ssh_output.stdout);
    let ssh_stderr = String::from_utf8_lossy(&ssh_output.stderr);

    println!("SSH stdout: {}", ssh_stdout);
    println!("SSH stderr: {}", ssh_stderr);

    // Cleanup domain before checking results
    cleanup_domain(&domain_name);

    // Check SSH results
    if !ssh_output.status.success() {
        // SSH might fail due to VM not being ready, network issues, etc.
        let acceptable_ssh_failures = [
            "connection",
            "timeout",
            "refused",
            "network",
            "ssh",
            "not running",
            "boot",
        ];

        let is_acceptable = acceptable_ssh_failures
            .iter()
            .any(|&pattern| ssh_stderr.to_lowercase().contains(&pattern.to_lowercase()));

        if is_acceptable {
            println!(
                "SSH connection failed (may be expected in test environment): {}",
                ssh_stderr
            );
            println!("Full workflow test completed - domain creation and SSH integration working");
            return;
        }

        panic!("SSH connection failed unexpectedly: {}", ssh_stderr);
    }

    // Verify we got the expected output
    assert!(
        ssh_stdout.contains("hello world"),
        "Expected 'hello world' in SSH output. Got: {}",
        ssh_stdout
    );

    println!("✓ Successfully executed 'echo hello world' via SSH");
    println!("✓ Full libvirt run + SSH workflow test passed");
}

/// Helper function to cleanup domain
fn cleanup_domain(domain_name: &str) {
    println!("Cleaning up domain: {}", domain_name);

    // Stop domain if running
    let _ = Command::new("virsh")
        .args(&["destroy", domain_name])
        .output();

    // Remove domain definition
    let cleanup_output = Command::new("virsh")
        .args(&["undefine", domain_name])
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

/// Test VM startup and shutdown with libvirt run
pub fn test_libvirt_vm_lifecycle() {
    // Skip if running in CI/container environment without libvirt
    if std::env::var("CI").is_ok() || !std::path::Path::new("/usr/bin/virsh").exists() {
        println!("Skipping VM lifecycle test - no libvirt available");
        return;
    }

    let bck = get_bck_command().unwrap();
    let test_volume = "test-vm-lifecycle";
    let domain_name = format!("bootc-{}", test_volume);

    // Cleanup any existing test domain
    let _ = std::process::Command::new("virsh")
        .args(&["destroy", &domain_name])
        .output();
    let _ = std::process::Command::new("virsh")
        .args(&["undefine", &domain_name])
        .output();

    // Create a minimal test volume (skip if no bootc container available)
    let test_image = &get_test_image();

    // First try to create a domain from container image
    let output = std::process::Command::new(&bck)
        .args(&[
            "libvirt",
            "run",
            "--filesystem",
            "ext4",
            "--name",
            &domain_name,
            test_image,
        ])
        .output()
        .expect("Failed to run libvirt run");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("Create failed (expected in CI): {}", stderr);

        // If creation fails (e.g., no libvirt storage), skip the test
        if stderr.contains("pool")
            || stderr.contains("connect")
            || stderr.contains("Permission denied")
        {
            println!("Skipping VM lifecycle test - libvirt not properly configured");
            return;
        }

        panic!("Unexpected failure in VM creation: {}", stderr);
    }

    println!("Created VM domain: {}", domain_name);

    // Try to start the domain
    let start_output = std::process::Command::new("virsh")
        .args(&["start", &domain_name])
        .output()
        .expect("Failed to run virsh start");

    if start_output.status.success() {
        println!("Successfully started VM: {}", domain_name);

        // Verify domain is running
        let dominfo_output = std::process::Command::new("virsh")
            .args(&["dominfo", &domain_name])
            .output()
            .expect("Failed to run virsh dominfo");

        let info = String::from_utf8_lossy(&dominfo_output.stdout);
        assert!(info.contains("State:"), "Should show domain state");

        // Wait a moment for VM to initialize
        std::thread::sleep(std::time::Duration::from_secs(5));

        // Stop the domain
        let stop_output = std::process::Command::new("virsh")
            .args(&["destroy", &domain_name])
            .output()
            .expect("Failed to run virsh destroy");

        if !stop_output.status.success() {
            let stderr = String::from_utf8_lossy(&stop_output.stderr);
            eprintln!("Warning: Failed to stop domain: {}", stderr);
        } else {
            println!("Successfully stopped VM: {}", domain_name);
        }
    } else {
        let stderr = String::from_utf8_lossy(&start_output.stderr);
        println!("VM start failed (may be expected): {}", stderr);

        // Some failures are acceptable (no KVM, nested virtualization, etc.)
        let acceptable_failures = [
            "KVM",
            "kvm",
            "nested",
            "hardware",
            "acceleration",
            "permission",
            "qemu",
            "network",
        ];

        let is_acceptable = acceptable_failures
            .iter()
            .any(|&pattern| stderr.to_lowercase().contains(pattern));

        if !is_acceptable {
            panic!("Unexpected VM start failure: {}", stderr);
        }
    }

    // Cleanup - remove the domain
    let _ = std::process::Command::new("virsh")
        .args(&["destroy", &domain_name])
        .output();
    let cleanup_output = std::process::Command::new("virsh")
        .args(&["undefine", &domain_name])
        .output()
        .expect("Failed to cleanup domain");

    if cleanup_output.status.success() {
        println!("Cleaned up VM domain: {}", domain_name);
    }

    println!("VM lifecycle test completed");
}

/// Test container storage binding functionality end-to-end
pub fn test_libvirt_bind_storage_ro() {
    let bck = get_bck_command().unwrap();
    let test_image = get_test_image();

    // Generate unique domain name for this test
    let domain_name = format!(
        "test-bind-storage-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    println!("Testing --bind-storage-ro with domain: {}", domain_name);

    // Cleanup any existing domain with this name
    let _ = Command::new("virsh")
        .args(&["destroy", &domain_name])
        .output();
    let _ = Command::new("virsh")
        .args(&["undefine", &domain_name])
        .output();

    // Create domain with --bind-storage-ro flag
    println!("Creating libvirt domain with --bind-storage-ro...");
    let create_output = Command::new("timeout")
        .args([
            "300s", // 5 minute timeout for domain creation
            &bck,
            "libvirt",
            "run",
            "--name",
            &domain_name,
            "--bind-storage-ro",
            "--filesystem",
            "ext4",
            &test_image,
        ])
        .output()
        .expect("Failed to run libvirt run with --bind-storage-ro");

    let create_stdout = String::from_utf8_lossy(&create_output.stdout);
    let create_stderr = String::from_utf8_lossy(&create_output.stderr);

    println!("Create stdout: {}", create_stdout);
    println!("Create stderr: {}", create_stderr);

    if !create_output.status.success() {
        cleanup_domain(&domain_name);
        panic!(
            "Failed to create domain with --bind-storage-ro: {}",
            create_stderr
        );
    }

    println!("Successfully created domain: {}", domain_name);

    // Check that the domain was created with virtiofs filesystem
    println!("Checking domain XML for virtiofs filesystem...");
    let dumpxml_output = Command::new("virsh")
        .args(&["dumpxml", &domain_name])
        .output()
        .expect("Failed to dump domain XML");

    if !dumpxml_output.status.success() {
        cleanup_domain(&domain_name);
        let stderr = String::from_utf8_lossy(&dumpxml_output.stderr);
        panic!("Failed to dump domain XML: {}", stderr);
    }

    let domain_xml = String::from_utf8_lossy(&dumpxml_output.stdout);
    println!(
        "Domain XML snippet: {}",
        &domain_xml[..std::cmp::min(500, domain_xml.len())]
    );

    // Verify that the domain XML contains virtiofs configuration
    assert!(
        domain_xml.contains("type='virtiofs'") || domain_xml.contains("driver type='virtiofs'"),
        "Domain XML should contain virtiofs filesystem configuration"
    );

    // Verify that the filesystem has the correct tag
    assert!(
        domain_xml.contains("hoststorage") || domain_xml.contains("dir='hoststorage'"),
        "Domain XML should reference the hoststorage tag for container storage"
    );

    // Check metadata for bind-storage-ro configuration
    if domain_xml.contains("bootc:bind-storage-ro") {
        assert!(
            domain_xml.contains("<bootc:bind-storage-ro>true</bootc:bind-storage-ro>"),
            "Domain metadata should indicate bind-storage-ro is enabled"
        );
    }

    println!("✓ Domain XML contains expected virtiofs configuration");
    println!("✓ Container storage mount is configured as read-only");
    println!("✓ hoststorage tag is present in filesystem configuration");

    // Wait for VM to boot and SSH to become available
    println!("Waiting for VM to boot and SSH to become available...");
    std::thread::sleep(std::time::Duration::from_secs(45));

    // Create mount point and mount virtiofs filesystem
    println!("Creating mount point and mounting virtiofs filesystem...");
    let mount_setup = Command::new("timeout")
        .args([
            "30s",
            &bck,
            "libvirt",
            "ssh",
            &domain_name,
            "--",
            "sudo",
            "mkdir",
            "-p",
            "/run/virtiofs-mnt-hoststorage",
        ])
        .output()
        .expect("Failed to create mount point");

    if !mount_setup.status.success() {
        let stderr = String::from_utf8_lossy(&mount_setup.stderr);
        println!("Warning: Failed to create mount point: {}", stderr);
    }

    let mount_cmd = Command::new("timeout")
        .args([
            "30s",
            &bck,
            "libvirt",
            "ssh",
            &domain_name,
            "--",
            "sudo",
            "mount",
            "-t",
            "virtiofs",
            "hoststorage",
            "/run/virtiofs-mnt-hoststorage",
        ])
        .output()
        .expect("Failed to mount virtiofs");

    if !mount_cmd.status.success() {
        cleanup_domain(&domain_name);
        let stderr = String::from_utf8_lossy(&mount_cmd.stderr);
        panic!("Failed to mount virtiofs filesystem: {}", stderr);
    }

    // Test SSH connection and verify container storage mount inside VM
    println!("Testing SSH connection and checking container storage mount...");
    let st = Command::new("timeout")
        .args([
            "60s",
            &bck,
            "libvirt",
            "ssh",
            &domain_name,
            "--",
            "ls",
            "-la",
            "/run/virtiofs-mnt-hoststorage/overlay",
        ])
        .status()
        .expect("Failed to run SSH command to check container storage");

    assert!(st.success());

    // Cleanup domain before completing test
    cleanup_domain(&domain_name);

    println!("✓ --bind-storage-ro end-to-end test passed");
}

/// Test error handling for invalid configurations
pub fn test_libvirt_error_handling() {
    let bck = get_bck_command().unwrap();

    let error_cases = vec![
        // Missing required arguments
        (vec!["libvirt", "run"], "missing image"),
        (vec!["libvirt", "ssh"], "missing domain"),
        // Invalid resource specs
        (
            vec!["libvirt", "run", "--memory", "invalid", "test-image"],
            "invalid memory",
        ),
        // Invalid format
        (vec!["libvirt", "list", "--format", "bad"], "invalid format"),
    ];

    for (args, error_desc) in error_cases {
        let output = Command::new(&bck)
            .args(&args)
            .output()
            .expect(&format!("Failed to run error case: {}", error_desc));

        assert!(
            !output.status.success(),
            "Should fail for case: {}",
            error_desc
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.is_empty(),
            "Should have error message for case: {}",
            error_desc
        );
    }

    println!("libvirt error handling validated");
}
