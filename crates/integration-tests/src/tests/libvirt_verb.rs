//! Integration tests for the libvirt verb with domain management subcommands
//!
//! These tests verify the libvirt command structure:
//! - `bcvk libvirt run` - Run bootable containers as persistent VMs
//! - `bcvk libvirt list` - List bootc domains
//! - `bcvk libvirt list-volumes` - List available bootc volumes
//! - `bcvk libvirt ssh` - SSH into domains
//! - Domain lifecycle management (start/stop/rm/inspect)

use std::process::Command;

use crate::{
    get_bck_command, get_test_image, run_bcvk, run_bcvk_nocapture, LIBVIRT_INTEGRATION_TEST_LABEL,
};

/// Test libvirt list functionality (lists domains)
pub fn test_libvirt_list_functionality() {
    let bck = get_bck_command().unwrap();

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
    let test_image = get_test_image();

    // Generate unique domain name for this test using shortuuid pattern
    let domain_name_template = "test-ssh-{shortuuid}";

    println!("Testing full libvirt run + SSH workflow");

    // Create temp file for domain name
    let id_file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let id_path = id_file.path().to_str().expect("Invalid temp file path");

    // Create domain with SSH key generation (name will be auto-generated)
    println!("Creating libvirt domain with SSH key injection...");
    let create_output = run_bcvk(&[
        "libvirt",
        "run",
        "--name",
        domain_name_template,
        "--write-id-to",
        id_path,
        "--label",
        LIBVIRT_INTEGRATION_TEST_LABEL,
        "--filesystem",
        "ext4",
        &test_image,
    ])
    .expect("Failed to run libvirt run with SSH");

    println!("Create stdout: {}", create_output.stdout);
    println!("Create stderr: {}", create_output.stderr);

    if !create_output.success() {
        // Attempt cleanup before panicking
        let _ = std::fs::read_to_string(id_path).map(|name| cleanup_domain(name.trim()));
        panic!("Failed to create domain with SSH: {}", create_output.stderr);
    }

    // Read the domain name from the file
    let domain_name = std::fs::read_to_string(id_path)
        .expect("Failed to read domain name from file")
        .trim()
        .to_string();

    println!("Successfully created domain: {}", domain_name);

    // Wait for VM to boot and SSH to become available
    println!("Waiting for VM to boot and SSH to become available...");
    std::thread::sleep(std::time::Duration::from_secs(30));

    // Test SSH connection with simple command
    println!("Testing SSH connection: echo 'hello world'");
    let ssh_output = run_bcvk(&["libvirt", "ssh", &domain_name, "--", "echo", "hello world"])
        .expect("Failed to run libvirt ssh command");

    println!("SSH stdout: {}", ssh_output.stdout);
    println!("SSH stderr: {}", ssh_output.stderr);

    // Cleanup domain before checking results
    cleanup_domain(&domain_name);

    // Check SSH results
    if !ssh_output.success() {
        panic!("SSH connection failed: {}", ssh_output.stderr);
    }

    // Verify we got the expected output
    assert!(
        ssh_output.stdout.contains("hello world"),
        "Expected 'hello world' in SSH output. Got: {}",
        ssh_output.stdout
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

/// Wait for SSH to become available on a domain with a timeout
fn wait_for_ssh_available(
    domain_name: &str,
    timeout_secs: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let start_time = std::time::Instant::now();
    let timeout_duration = std::time::Duration::from_secs(timeout_secs);

    println!(
        "Waiting for SSH to become available on domain: {}",
        domain_name
    );

    loop {
        // Try a simple SSH command to test connectivity
        let ssh_test = run_bcvk(&["libvirt", "ssh", domain_name, "--", "echo", "ssh-ready"]);

        match ssh_test {
            Ok(output) if output.success() => {
                println!("✓ SSH is now available");
                return Ok(());
            }
            Ok(_) => {
                // SSH command failed, but that's expected while VM is booting
            }
            Err(e) => {
                println!("SSH test error (expected while booting): {}", e);
            }
        }

        // Check if we've exceeded the timeout
        if start_time.elapsed() >= timeout_duration {
            return Err(format!("Timeout waiting for SSH after {} seconds", timeout_secs).into());
        }

        // Wait 5 seconds before next attempt
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
}

/// Test VM startup and shutdown with libvirt run
pub fn test_libvirt_vm_lifecycle() {
    let bck = get_bck_command().unwrap();
    let domain_name_template = "bootc-lifecycle-{shortuuid}";

    // Create a minimal test volume (skip if no bootc container available)
    let test_image = &get_test_image();

    // Create temp file for domain name
    let id_file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let id_path = id_file.path().to_str().expect("Invalid temp file path");

    // First try to create a domain from container image
    let output = std::process::Command::new(&bck)
        .args(&[
            "libvirt",
            "run",
            "--filesystem",
            "ext4",
            "--name",
            domain_name_template,
            "--write-id-to",
            id_path,
            "--label",
            LIBVIRT_INTEGRATION_TEST_LABEL,
            test_image,
        ])
        .output()
        .expect("Failed to run libvirt run");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("Failed to create VM: {}", stderr);
    }

    // Read the domain name from the file
    let domain_name = std::fs::read_to_string(id_path)
        .expect("Failed to read domain name from file")
        .trim()
        .to_string();

    println!("Created VM domain: {}", domain_name);

    // Guard to ensure cleanup always runs
    struct VmCleanupGuard {
        domain_name: String,
        bck: String,
    }
    impl Drop for VmCleanupGuard {
        fn drop(&mut self) {
            // Try to stop the VM first
            let _ = std::process::Command::new("virsh")
                .args(&["destroy", &self.domain_name])
                .output();
            // Use bcvk libvirt rm for cleanup
            let cleanup_output = std::process::Command::new(&self.bck)
                .args(&["libvirt", "rm", &self.domain_name, "--force", "--stop"])
                .output();
            if let Ok(output) = cleanup_output {
                if output.status.success() {
                    println!("Cleaned up VM domain: {}", self.domain_name);
                }
            }
        }
    }

    // Set up cleanup guard after successful creation
    let _guard = VmCleanupGuard {
        domain_name: domain_name.clone(),
        bck: bck.clone(),
    };

    // Verify domain is running (libvirt run starts the domain by default)
    let dominfo_output = std::process::Command::new("virsh")
        .args(&["dominfo", &domain_name])
        .output()
        .expect("Failed to run virsh dominfo");

    let info = String::from_utf8_lossy(&dominfo_output.stdout);
    assert!(info.contains("State:"), "Should show domain state");
    assert!(
        info.contains("running") || info.contains("idle"),
        "Domain should be running after creation"
    );
    println!("Verified VM is running: {}", domain_name);

    // Wait a moment for VM to initialize
    std::thread::sleep(std::time::Duration::from_secs(5));

    // Stop the domain
    let stop_output = std::process::Command::new("virsh")
        .args(&["destroy", &domain_name])
        .output()
        .expect("Failed to run virsh destroy");

    if !stop_output.status.success() {
        let stderr = String::from_utf8_lossy(&stop_output.stderr);
        panic!("Failed to stop domain: {}", stderr);
    }
    println!("Successfully stopped VM: {}", domain_name);

    println!("VM lifecycle test completed");
}

/// Test container storage binding functionality end-to-end
pub fn test_libvirt_bind_storage_ro() {
    let bck = get_bck_command().unwrap();
    let test_image = get_test_image();

    // First check if libvirt supports readonly virtiofs
    println!("Checking libvirt capabilities...");
    let status_output = Command::new(&bck)
        .args(&["libvirt", "status", "--format", "json"])
        .output()
        .expect("Failed to get libvirt status");

    if !status_output.status.success() {
        let stderr = String::from_utf8_lossy(&status_output.stderr);
        panic!("Failed to get libvirt status: {}", stderr);
    }

    let status: serde_json::Value =
        serde_json::from_slice(&status_output.stdout).expect("Failed to parse libvirt status JSON");

    let supports_readonly = status["supports_readonly_virtiofs"]
        .as_bool()
        .expect("Missing supports_readonly_virtiofs field in status output");

    if !supports_readonly {
        println!("Skipping test: libvirt does not support readonly virtiofs");
        println!("libvirt version: {:?}", status["version"]);
        println!("Requires libvirt 11.0+ for readonly virtiofs support");
        return;
    }

    // Generate unique domain name for this test using shortuuid pattern
    let domain_name_template = "test-bind-storage-{shortuuid}";

    println!("Testing --bind-storage-ro");

    // Create temp file for domain name
    let id_file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let id_path = id_file.path().to_str().expect("Invalid temp file path");

    // Create domain with --bind-storage-ro flag
    println!("Creating libvirt domain with --bind-storage-ro...");
    let create_output = run_bcvk(&[
        "libvirt",
        "run",
        "--name",
        domain_name_template,
        "--write-id-to",
        id_path,
        "--label",
        LIBVIRT_INTEGRATION_TEST_LABEL,
        "--bind-storage-ro",
        "--filesystem",
        "ext4",
        &test_image,
    ])
    .expect("Failed to run libvirt run with --bind-storage-ro");

    println!("Create stdout: {}", create_output.stdout);
    println!("Create stderr: {}", create_output.stderr);

    if !create_output.success() {
        // Attempt cleanup before panicking
        let _ = std::fs::read_to_string(id_path).map(|name| cleanup_domain(name.trim()));
        panic!(
            "Failed to create domain with --bind-storage-ro: {}",
            create_output.stderr
        );
    }

    // Read the domain name from the file
    let domain_name = std::fs::read_to_string(id_path)
        .expect("Failed to read domain name from file")
        .trim()
        .to_string();

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

    // Verify that the domain XML contains readonly element for virtiofs
    assert!(
        domain_xml.contains("<readonly/>"),
        "Domain XML should contain readonly element for --bind-storage-ro"
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
    if let Err(e) = wait_for_ssh_available(&domain_name, 180) {
        cleanup_domain(&domain_name);
        panic!("Failed to establish SSH connection: {}", e);
    }

    // Create mount point and mount virtiofs filesystem
    println!("Creating mount point and mounting virtiofs filesystem...");
    let mount_setup = run_bcvk(&[
        "libvirt",
        "ssh",
        &domain_name,
        "--",
        "sudo",
        "mkdir",
        "-p",
        "/run/virtiofs-mnt-hoststorage",
    ])
    .expect("Failed to create mount point");

    if !mount_setup.success() {
        println!(
            "Warning: Failed to create mount point: {}",
            mount_setup.stderr
        );
    }

    let mount_cmd = run_bcvk(&[
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
    .expect("Failed to mount virtiofs");

    if !mount_cmd.success() {
        cleanup_domain(&domain_name);
        panic!("Failed to mount virtiofs filesystem: {}", mount_cmd.stderr);
    }

    // Test SSH connection and verify container storage mount inside VM
    println!("Testing SSH connection and checking container storage mount...");
    run_bcvk_nocapture(&[
        "libvirt",
        "ssh",
        &domain_name,
        "--",
        "ls",
        "-la",
        "/run/virtiofs-mnt-hoststorage/overlay",
    ])
    .expect("Failed to run SSH command to check container storage");

    // Verify that the mount is read-only
    println!("Verifying that the mount is read-only...");
    let ro_test_st = run_bcvk(&[
        "libvirt",
        "ssh",
        &domain_name,
        "--",
        "touch",
        "/run/virtiofs-mnt-hoststorage/test-write",
    ])
    .expect("Failed to run SSH command to test read-only mount");

    assert!(
        !ro_test_st.success(),
        "Mount should be read-only, but write operation succeeded"
    );
    println!("✓ Mount is correctly configured as read-only.");

    // Cleanup domain before completing test
    cleanup_domain(&domain_name);

    println!("✓ --bind-storage-ro end-to-end test passed");
}

/// Test libvirt label functionality
pub fn test_libvirt_label_functionality() {
    let bck = get_bck_command().unwrap();
    let test_image = get_test_image();

    // Generate unique domain name for this test using shortuuid pattern
    let domain_name_template = "test-label-{shortuuid}";

    println!("Testing libvirt label functionality");

    // Create temp file for domain name
    let id_file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let id_path = id_file.path().to_str().expect("Invalid temp file path");

    // Create domain with multiple labels
    println!("Creating libvirt domain with multiple labels...");
    let create_output = run_bcvk(&[
        "libvirt",
        "run",
        "--name",
        domain_name_template,
        "--write-id-to",
        id_path,
        "--label",
        LIBVIRT_INTEGRATION_TEST_LABEL,
        "--label",
        "test-env",
        "--label",
        "temporary",
        "--filesystem",
        "ext4",
        &test_image,
    ])
    .expect("Failed to run libvirt run with labels");

    println!("Create stdout: {}", create_output.stdout);
    println!("Create stderr: {}", create_output.stderr);

    if !create_output.success() {
        // Attempt cleanup before panicking
        let _ = std::fs::read_to_string(id_path).map(|name| cleanup_domain(name.trim()));
        panic!(
            "Failed to create domain with labels: {}",
            create_output.stderr
        );
    }

    // Read the domain name from the file
    let domain_name = std::fs::read_to_string(id_path)
        .expect("Failed to read domain name from file")
        .trim()
        .to_string();

    println!("Successfully created domain with labels: {}", domain_name);

    // Verify labels are stored in domain XML
    println!("Checking domain XML for labels...");
    let dumpxml_output = Command::new("virsh")
        .args(&["dumpxml", &domain_name])
        .output()
        .expect("Failed to dump domain XML");

    let domain_xml = String::from_utf8_lossy(&dumpxml_output.stdout);

    // Check that labels are in the XML
    assert!(
        domain_xml.contains("bootc:label") || domain_xml.contains("<label>"),
        "Domain XML should contain label metadata"
    );
    assert!(
        domain_xml.contains(LIBVIRT_INTEGRATION_TEST_LABEL),
        "Domain XML should contain bcvk-integration label"
    );

    // Test filtering by label
    println!("Testing label filtering with libvirt list...");
    let list_output = Command::new(&bck)
        .args([
            "libvirt",
            "list",
            "--label",
            LIBVIRT_INTEGRATION_TEST_LABEL,
            "-a",
        ])
        .output()
        .expect("Failed to run libvirt list with label filter");

    let list_stdout = String::from_utf8_lossy(&list_output.stdout);
    println!("List output: {}", list_stdout);

    assert!(
        list_output.status.success(),
        "libvirt list with label filter should succeed"
    );
    assert!(
        list_stdout.contains(&domain_name),
        "Domain should appear in filtered list. Output: {}",
        list_stdout
    );

    // Test filtering by a label that should match
    let list_test_env = Command::new(&bck)
        .args(["libvirt", "list", "--label", "test-env", "-a"])
        .output()
        .expect("Failed to run libvirt list with test-env label");

    let list_test_env_stdout = String::from_utf8_lossy(&list_test_env.stdout);
    assert!(
        list_test_env_stdout.contains(&domain_name),
        "Domain should appear when filtering by test-env label"
    );

    // Test filtering by a label that should NOT match
    let list_nomatch = Command::new(&bck)
        .args(["libvirt", "list", "--label", "nonexistent-label", "-a"])
        .output()
        .expect("Failed to run libvirt list with nonexistent label");

    let list_nomatch_stdout = String::from_utf8_lossy(&list_nomatch.stdout);
    assert!(
        !list_nomatch_stdout.contains(&domain_name),
        "Domain should NOT appear when filtering by nonexistent label"
    );

    // Cleanup domain
    cleanup_domain(&domain_name);

    println!("✓ Label functionality test passed");
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
