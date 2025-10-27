//! Integration tests for the libvirt verb with domain management subcommands
//!
//! These tests verify the libvirt command structure:
//! - `bcvk libvirt run` - Run bootable containers as persistent VMs
//! - `bcvk libvirt list` - List bootc domains
//! - `bcvk libvirt list-volumes` - List available bootc volumes
//! - `bcvk libvirt ssh` - SSH into domains
//! - Domain lifecycle management (start/stop/rm/inspect)

use color_eyre::Result;
use linkme::distributed_slice;
use std::process::Command;

use crate::{
    get_bck_command, get_test_image, run_bcvk, run_bcvk_nocapture, IntegrationTest,
    INTEGRATION_TESTS, LIBVIRT_INTEGRATION_TEST_LABEL,
};
use bcvk::xml_utils::parse_xml_dom;

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_LIST_FUNCTIONALITY: IntegrationTest = IntegrationTest::new(
    "libvirt_list_functionality",
    test_libvirt_list_functionality,
);

/// Test libvirt list functionality (lists domains)
fn test_libvirt_list_functionality() -> Result<()> {
    let bck = get_bck_command()?;

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
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_LIST_JSON_OUTPUT: IntegrationTest =
    IntegrationTest::new("libvirt_list_json_output", test_libvirt_list_json_output);

/// Test libvirt list with JSON output
fn test_libvirt_list_json_output() -> Result<()> {
    let bck = get_bck_command()?;

    let output = Command::new(&bck)
        .args(["libvirt", "list", "--format", "json"])
        .output()
        .expect("Failed to run libvirt list --format json");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        // If successful, should be valid JSON
        let json_result: std::result::Result<serde_json::Value, _> = serde_json::from_str(&stdout);
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
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_LIST_JSON_SSH_METADATA: IntegrationTest = IntegrationTest::new(
    "test_libvirt_list_json_ssh_metadata",
    test_libvirt_list_json_ssh_metadata,
);

/// Test libvirt list JSON output includes SSH metadata
fn test_libvirt_list_json_ssh_metadata() -> Result<()> {
    let test_image = get_test_image();

    // Generate unique domain name for this test
    let domain_name = format!(
        "test-json-ssh-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    println!(
        "Testing libvirt list JSON output with SSH metadata for domain: {}",
        domain_name
    );

    // Cleanup any existing domain with this name
    cleanup_domain(&domain_name);

    // Create domain with SSH key generation (default behavior)
    println!("Creating libvirt domain with SSH key...");
    let create_output = run_bcvk(&[
        "libvirt",
        "run",
        "--name",
        &domain_name,
        "--label",
        LIBVIRT_INTEGRATION_TEST_LABEL,
        "--filesystem",
        "ext4",
        &test_image,
    ])
    .expect("Failed to run libvirt run");

    println!("Create stdout: {}", create_output.stdout);
    println!("Create stderr: {}", create_output.stderr);

    if !create_output.success() {
        cleanup_domain(&domain_name);
        panic!("Failed to create domain with SSH: {}", create_output.stderr);
    }

    println!("Successfully created domain: {}", domain_name);

    // List domains with JSON format
    println!("Listing domains with JSON format...");
    let bck = get_bck_command()?;
    let list_output = Command::new(&bck)
        .args(["libvirt", "list", "--format", "json", "-a"])
        .output()
        .expect("Failed to run libvirt list --format json");

    let list_stdout = String::from_utf8_lossy(&list_output.stdout);
    println!("List JSON output: {}", list_stdout);

    // Cleanup domain before assertions
    cleanup_domain(&domain_name);

    // Check that the command succeeded
    if !list_output.status.success() {
        let stderr = String::from_utf8_lossy(&list_output.stderr);
        panic!("libvirt list --format json failed: {}", stderr);
    }

    // Parse JSON output
    let domains: Vec<serde_json::Value> =
        serde_json::from_str(&list_stdout).expect("Failed to parse JSON output from libvirt list");

    // Find our test domain in the output
    let test_domain = domains
        .iter()
        .find(|d| d["name"].as_str() == Some(&domain_name))
        .expect(&format!(
            "Test domain '{}' not found in JSON output",
            domain_name
        ));

    println!("Found test domain in JSON output: {:?}", test_domain);

    // Verify SSH port is present and is a number
    let ssh_port = test_domain["ssh_port"]
        .as_u64()
        .expect("ssh_port should be present and be a number");
    assert!(
        ssh_port > 0 && ssh_port < 65536,
        "ssh_port should be a valid port number, got: {}",
        ssh_port
    );
    println!("✓ ssh_port is present and valid: {}", ssh_port);

    // Verify has_ssh_key is true
    let has_ssh_key = test_domain["has_ssh_key"]
        .as_bool()
        .expect("has_ssh_key should be present and be a boolean");
    assert!(
        has_ssh_key,
        "has_ssh_key should be true for domain created with SSH key"
    );
    println!("✓ has_ssh_key is true");

    // Verify ssh_private_key is present and looks like a valid SSH key
    let ssh_private_key = test_domain["ssh_private_key"]
        .as_str()
        .expect("ssh_private_key should be present and be a string");
    assert!(
        !ssh_private_key.is_empty(),
        "ssh_private_key should not be empty"
    );
    assert!(
        ssh_private_key.contains("-----BEGIN") && ssh_private_key.contains("PRIVATE KEY-----"),
        "ssh_private_key should be a valid SSH private key format, got: {}",
        &ssh_private_key[..std::cmp::min(100, ssh_private_key.len())]
    );
    assert!(
        ssh_private_key.contains("-----END") && ssh_private_key.contains("PRIVATE KEY-----"),
        "ssh_private_key should have proper end marker"
    );

    // Verify the key has proper newlines (not escaped \n)
    assert!(
        ssh_private_key.lines().count() > 1,
        "ssh_private_key should have multiple lines, not escaped newlines"
    );

    println!(
        "✓ ssh_private_key is present and valid (has {} lines)",
        ssh_private_key.lines().count()
    );

    println!("✓ libvirt list JSON SSH metadata test passed");
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_RUN_RESOURCE_OPTIONS: IntegrationTest = IntegrationTest::new(
    "test_libvirt_run_resource_options",
    test_libvirt_run_resource_options,
);

/// Test domain resource configuration options
fn test_libvirt_run_resource_options() -> Result<()> {
    let bck = get_bck_command()?;

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
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_RUN_NETWORKING: IntegrationTest =
    IntegrationTest::new("test_libvirt_run_networking", test_libvirt_run_networking);

/// Test domain networking configuration
fn test_libvirt_run_networking() -> Result<()> {
    let bck = get_bck_command()?;

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
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_SSH_INTEGRATION: IntegrationTest =
    IntegrationTest::new("test_libvirt_ssh_integration", test_libvirt_ssh_integration);

/// Test SSH integration with created domains (syntax only)
fn test_libvirt_ssh_integration() -> Result<()> {
    let bck = get_bck_command()?;

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
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_RUN_SSH_FULL_WORKFLOW: IntegrationTest = IntegrationTest::new(
    "test_libvirt_run_ssh_full_workflow",
    test_libvirt_run_ssh_full_workflow,
);

/// Test full libvirt run + SSH workflow like run_ephemeral SSH tests
fn test_libvirt_run_ssh_full_workflow() -> Result<()> {
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
    cleanup_domain(&domain_name);

    // Create domain with SSH key generation
    println!("Creating libvirt domain with SSH key injection...");
    let create_output = run_bcvk(&[
        "libvirt",
        "run",
        "--name",
        &domain_name,
        "--label",
        LIBVIRT_INTEGRATION_TEST_LABEL,
        "--filesystem",
        "ext4",
        "--karg",
        "bcvk.test-install-karg=1",
        &test_image,
    ])
    .expect("Failed to run libvirt run with SSH");

    println!("Create stdout: {}", create_output.stdout);
    println!("Create stderr: {}", create_output.stderr);

    if !create_output.success() {
        cleanup_domain(&domain_name);

        panic!("Failed to create domain with SSH: {}", create_output.stderr);
    }

    println!("Successfully created domain: {}", domain_name);

    // Wait for VM to boot and SSH to become available
    println!("Waiting for VM to boot and SSH to become available...");
    std::thread::sleep(std::time::Duration::from_secs(30));

    // Test SSH connection and read kernel command line
    println!("Testing SSH connection and validating karg");
    let ssh_output = run_bcvk(&["libvirt", "ssh", &domain_name, "--", "cat", "/proc/cmdline"])
        .expect("Failed to run libvirt ssh command");

    println!("SSH stdout: {}", ssh_output.stdout);
    println!("SSH stderr: {}", ssh_output.stderr);

    // Cleanup domain before checking results
    cleanup_domain(&domain_name);

    // Check SSH results
    if !ssh_output.success() {
        panic!(
            "SSH connection failed: {}\nkernel cmdline: {}",
            ssh_output.stderr, ssh_output.stdout
        );
    }

    // Verify we got the expected karg in /proc/cmdline
    assert!(
        ssh_output.stdout.contains("bcvk.test-install-karg=1"),
        "Expected bcvk.test-install-karg=1 in kernel cmdline.\nActual cmdline: {}",
        ssh_output.stdout
    );

    println!("✓ Full libvirt run + SSH workflow test passed");
    Ok(())
}

/// Helper function to cleanup domain
fn cleanup_domain(domain_name: &str) {
    println!("Cleaning up domain: {}", domain_name);

    // Stop domain if running
    let _ = Command::new("virsh")
        .args(&["destroy", domain_name])
        .output();

    // Use bcvk libvirt rm for proper cleanup
    let bck = match get_bck_command() {
        Ok(cmd) => cmd,
        Err(_) => return,
    };
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

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_VM_LIFECYCLE: IntegrationTest =
    IntegrationTest::new("test_libvirt_vm_lifecycle", test_libvirt_vm_lifecycle);

/// Test VM startup and shutdown with libvirt run
fn test_libvirt_vm_lifecycle() -> Result<()> {
    let bck = get_bck_command()?;
    let test_volume = "test-vm-lifecycle";
    let domain_name = format!("bootc-{}", test_volume);

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

    // Cleanup any existing test domain
    let _ = std::process::Command::new("virsh")
        .args(&["destroy", &domain_name])
        .output();
    let _ = std::process::Command::new(&bck)
        .args(&["libvirt", "rm", &domain_name, "--force", "--stop"])
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

    println!("Created VM domain: {}", domain_name);

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
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_BIND_STORAGE_RO: IntegrationTest =
    IntegrationTest::new("test_libvirt_bind_storage_ro", test_libvirt_bind_storage_ro);

/// Test container storage binding functionality end-to-end
fn test_libvirt_bind_storage_ro() -> Result<()> {
    let bck = get_bck_command()?;
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
        return Ok(());
    }

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
    cleanup_domain(&domain_name);

    // Create domain with --bind-storage-ro flag
    println!("Creating libvirt domain with --bind-storage-ro...");
    let create_output = run_bcvk(&[
        "libvirt",
        "run",
        "--name",
        &domain_name,
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
        cleanup_domain(&domain_name);
        panic!(
            "Failed to create domain with --bind-storage-ro: {}",
            create_output.stderr
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

    // Wait for VM to boot and automatic mount to complete
    println!("Waiting for VM to boot and automatic mount to complete...");
    std::thread::sleep(std::time::Duration::from_secs(10));

    // Test SSH connection and verify container storage is automatically mounted
    println!(
        "Verifying container storage is automatically mounted at /run/host-container-storage..."
    );
    run_bcvk_nocapture(&[
        "libvirt",
        "ssh",
        &domain_name,
        "--",
        "ls",
        "-la",
        "/run/host-container-storage/overlay",
    ])
    .expect("Failed to verify automatic mount of container storage");

    // Verify that the mount is read-only
    println!("Verifying that the mount is read-only...");
    let ro_test_st = run_bcvk(&[
        "libvirt",
        "ssh",
        &domain_name,
        "--",
        "touch",
        "/run/host-container-storage/test-write",
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
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_LABEL_FUNCTIONALITY: IntegrationTest = IntegrationTest::new(
    "test_libvirt_label_functionality",
    test_libvirt_label_functionality,
);

/// Test libvirt label functionality
fn test_libvirt_label_functionality() -> Result<()> {
    let bck = get_bck_command()?;
    let test_image = get_test_image();

    // Generate unique domain name for this test
    let domain_name = format!(
        "test-label-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    println!(
        "Testing libvirt label functionality with domain: {}",
        domain_name
    );

    // Cleanup any existing domain with this name
    cleanup_domain(&domain_name);

    // Create domain with multiple labels
    println!("Creating libvirt domain with multiple labels...");
    let create_output = run_bcvk(&[
        "libvirt",
        "run",
        "--name",
        &domain_name,
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
        cleanup_domain(&domain_name);
        panic!(
            "Failed to create domain with labels: {}",
            create_output.stderr
        );
    }

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
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_ERROR_HANDLING: IntegrationTest =
    IntegrationTest::new("test_libvirt_error_handling", test_libvirt_error_handling);

/// Test error handling for invalid configurations
fn test_libvirt_error_handling() -> Result<()> {
    let bck = get_bck_command()?;

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
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_TRANSIENT_VM: IntegrationTest =
    IntegrationTest::new("test_libvirt_transient_vm", test_libvirt_transient_vm);

/// Test transient VM functionality
fn test_libvirt_transient_vm() -> Result<()> {
    let test_image = get_test_image();

    // Generate unique domain name for this test
    let domain_name = format!(
        "test-transient-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    println!("Testing transient VM with domain: {}", domain_name);

    // Cleanup any existing domain with this name
    cleanup_domain(&domain_name);

    // Create transient domain
    println!("Creating transient libvirt domain...");
    let create_output = run_bcvk(&[
        "libvirt",
        "run",
        "--name",
        &domain_name,
        "--label",
        LIBVIRT_INTEGRATION_TEST_LABEL,
        "--transient",
        "--filesystem",
        "ext4",
        &test_image,
    ])
    .expect("Failed to run libvirt run with --transient");

    println!("Create stdout: {}", create_output.stdout);
    println!("Create stderr: {}", create_output.stderr);

    if !create_output.success() {
        cleanup_domain(&domain_name);
        panic!(
            "Failed to create transient domain: {}",
            create_output.stderr
        );
    }

    println!("Successfully created transient domain: {}", domain_name);

    // Verify domain is transient using virsh dominfo
    println!("Verifying domain is marked as transient...");
    let dominfo_output = Command::new("virsh")
        .args(&["dominfo", &domain_name])
        .output()
        .expect("Failed to run virsh dominfo");

    if !dominfo_output.status.success() {
        cleanup_domain(&domain_name);
        let stderr = String::from_utf8_lossy(&dominfo_output.stderr);
        panic!("Failed to get domain info: {}", stderr);
    }

    let dominfo = String::from_utf8_lossy(&dominfo_output.stdout);
    println!("Domain info:\n{}", dominfo);

    // Verify "Persistent: no" appears in dominfo
    assert!(
        dominfo.contains("Persistent:") && dominfo.contains("no"),
        "Domain should be marked as non-persistent (transient). dominfo: {}",
        dominfo
    );
    println!("✓ Domain is correctly marked as transient (Persistent: no)");

    // Verify domain XML contains transient disk element
    println!("Checking domain XML for transient disk configuration...");
    let dumpxml_output = Command::new("virsh")
        .args(&["dumpxml", &domain_name])
        .output()
        .expect("Failed to dump domain XML");

    let domain_xml = String::from_utf8_lossy(&dumpxml_output.stdout);

    // Parse the XML properly using our XML parser
    let xml_dom = parse_xml_dom(&domain_xml).expect("Failed to parse domain XML");

    // Verify domain XML contains transient disk element
    let has_transient = xml_dom.find("transient").is_some();
    assert!(
        has_transient,
        "Domain XML should contain transient disk element"
    );
    println!("✓ Domain XML contains transient disk element");

    // Extract the base disk path from the domain XML using proper XML parsing
    let base_disk_path = xml_dom
        .find("source")
        .and_then(|source_node| source_node.attributes.get("file"))
        .map(|s| s.to_string());

    println!("Base disk path: {:?}", base_disk_path);

    // Stop the domain (this should make it disappear since it's transient)
    println!("Stopping transient domain (should disappear)...");
    let destroy_output = Command::new("virsh")
        .args(&["destroy", &domain_name])
        .output()
        .expect("Failed to run virsh destroy");

    if !destroy_output.status.success() {
        let stderr = String::from_utf8_lossy(&destroy_output.stderr);
        panic!("Failed to stop domain: {}", stderr);
    }

    // Poll for domain disappearance with timeout
    println!("Verifying domain has disappeared...");
    let start_time = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(10);
    let mut domain_disappeared = false;

    while start_time.elapsed() < timeout {
        let list_output = Command::new("virsh")
            .args(&["list", "--all", "--name"])
            .output()
            .expect("Failed to list domains");

        let domain_list = String::from_utf8_lossy(&list_output.stdout);
        if !domain_list.contains(&domain_name) {
            domain_disappeared = true;
            break;
        }

        // Wait briefly before checking again
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    assert!(
        domain_disappeared,
        "Transient domain should have disappeared after shutdown within {} seconds",
        timeout.as_secs()
    );
    println!("✓ Transient domain disappeared after shutdown");

    // Verify base disk still exists (only the overlay was removed)
    if let Some(ref disk_path) = base_disk_path {
        println!("Verifying base disk still exists: {}", disk_path);
        let disk_exists = std::path::Path::new(disk_path).exists();
        assert!(
            disk_exists,
            "Base disk should still exist after transient domain shutdown"
        );
        println!("✓ Base disk still exists (not deleted)");
    }

    println!("✓ Transient VM test passed");
    Ok(())
}

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_BIND_MOUNTS: IntegrationTest =
    IntegrationTest::new("test_libvirt_bind_mounts", test_libvirt_bind_mounts);

/// Test automatic bind mount functionality with systemd mount units
fn test_libvirt_bind_mounts() -> Result<()> {
    use camino::Utf8Path;
    use std::fs;
    use tempfile::TempDir;

    let test_image = get_test_image();

    // Generate unique domain name for this test
    let domain_name = format!(
        "test-bind-mounts-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    println!("Testing bind mounts with domain: {}", domain_name);

    // Create temporary directories for testing bind mounts
    let rw_dir = TempDir::new().expect("Failed to create read-write temp directory");
    let rw_dir_path = Utf8Path::from_path(rw_dir.path()).expect("rw dir path is not utf8");
    let rw_test_file = rw_dir_path.join("rw-test.txt");
    fs::write(&rw_test_file, "read-write content").expect("Failed to write rw test file");

    let ro_dir = TempDir::new().expect("Failed to create read-only temp directory");
    let ro_dir_path = Utf8Path::from_path(ro_dir.path()).expect("ro dir path is not utf8");
    let ro_test_file = ro_dir_path.join("ro-test.txt");
    fs::write(&ro_test_file, "read-only content").expect("Failed to write ro test file");

    println!("RW directory: {}", rw_dir_path);
    println!("RO directory: {}", ro_dir_path);

    // Cleanup any existing domain with this name
    cleanup_domain(&domain_name);

    // Create domain with bind mounts
    println!("Creating libvirt domain with bind mounts...");
    let create_output = run_bcvk(&[
        "libvirt",
        "run",
        "--name",
        &domain_name,
        "--label",
        LIBVIRT_INTEGRATION_TEST_LABEL,
        "--filesystem",
        "ext4",
        "--bind",
        &format!("{}:/var/mnt/test-rw", rw_dir_path),
        "--bind-ro",
        &format!("{}:/var/mnt/test-ro", ro_dir_path),
        &test_image,
    ])
    .expect("Failed to run libvirt run with bind mounts");

    println!("Create stdout: {}", create_output.stdout);
    println!("Create stderr: {}", create_output.stderr);

    if !create_output.success() {
        cleanup_domain(&domain_name);
        panic!(
            "Failed to create domain with bind mounts: {}",
            create_output.stderr
        );
    }

    println!("Successfully created domain: {}", domain_name);

    // Check domain XML for virtiofs filesystems and SMBIOS credentials
    println!("Checking domain XML for virtiofs and SMBIOS credentials...");
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

    // Verify virtiofs filesystems are present
    assert!(
        domain_xml.contains("type='virtiofs'") || domain_xml.contains("driver type='virtiofs'"),
        "Domain XML should contain virtiofs filesystem configuration"
    );

    // Verify SMBIOS credentials are injected
    assert!(
        domain_xml.contains("systemd.extra-unit"),
        "Domain XML should contain systemd.extra-unit SMBIOS credentials for mount units"
    );

    println!("✓ Domain XML contains virtiofs and SMBIOS credentials");

    // Wait for VM to boot and mounts to be ready
    println!("Waiting for VM to boot and mounts to be ready...");
    std::thread::sleep(std::time::Duration::from_secs(15));

    // Debug: Check systemd credentials
    println!("Debugging: Checking systemd credentials...");
    let _creds_check = run_bcvk(&[
        "libvirt",
        "ssh",
        &domain_name,
        "--",
        "ls",
        "-la",
        "/run/credentials",
    ])
    .expect("Failed to check credentials");

    // Debug: Check mount units
    println!("Debugging: Checking mount units...");
    let _units_check = run_bcvk(&[
        "libvirt",
        "ssh",
        &domain_name,
        "--",
        "systemctl",
        "list-units",
        "*.mount",
    ])
    .expect("Failed to check mount units");

    // Debug: Check mount status
    println!("Debugging: Checking if mounts exist...");
    let _mount_check = run_bcvk(&[
        "libvirt",
        "ssh",
        &domain_name,
        "--",
        "mount",
        "|",
        "grep",
        "virtiofs",
    ])
    .expect("Failed to check mounts");

    // Test read-write bind mount - verify file exists and is readable
    println!("Testing read-write bind mount...");
    let rw_read_test = run_bcvk(&[
        "libvirt",
        "ssh",
        &domain_name,
        "--",
        "cat",
        "/var/mnt/test-rw/rw-test.txt",
    ])
    .expect("Failed to read from rw bind mount");

    assert!(
        rw_read_test.success(),
        "Should be able to read from rw bind mount. stderr: {}",
        rw_read_test.stderr
    );
    assert!(
        rw_read_test.stdout.contains("read-write content"),
        "Should read correct content from rw bind mount"
    );
    println!("✓ RW bind mount is readable");

    // Test write access on read-write mount
    println!("Testing write access on read-write bind mount...");
    let rw_write_test = run_bcvk(&[
        "libvirt",
        "ssh",
        &domain_name,
        "--",
        "sh",
        "-c",
        "echo 'new content' > /var/mnt/test-rw/write-test.txt",
    ])
    .expect("Failed to write to rw bind mount");

    assert!(
        rw_write_test.success(),
        "Should be able to write to rw bind mount. stderr: {}",
        rw_write_test.stderr
    );
    println!("✓ RW bind mount is writable");

    // Verify written file exists on host
    let written_file = rw_dir_path.join("write-test.txt");
    assert!(
        written_file.exists(),
        "Written file should exist on host filesystem"
    );
    println!("✓ Written file exists on host");

    // Test read-only bind mount - verify file exists and is readable
    println!("Testing read-only bind mount...");
    let ro_read_test = run_bcvk(&[
        "libvirt",
        "ssh",
        &domain_name,
        "--",
        "cat",
        "/var/mnt/test-ro/ro-test.txt",
    ])
    .expect("Failed to read from ro bind mount");

    assert!(
        ro_read_test.success(),
        "Should be able to read from ro bind mount. stderr: {}",
        ro_read_test.stderr
    );
    assert!(
        ro_read_test.stdout.contains("read-only content"),
        "Should read correct content from ro bind mount"
    );
    println!("✓ RO bind mount is readable");

    // Test that read-only mount rejects writes
    println!("Testing that read-only bind mount rejects writes...");
    let ro_write_test = run_bcvk(&[
        "libvirt",
        "ssh",
        &domain_name,
        "--",
        "sh",
        "-c",
        "echo 'should fail' > /var/mnt/test-ro/write-test.txt 2>&1",
    ])
    .expect("Failed to test write to ro bind mount");

    assert!(
        !ro_write_test.success(),
        "Write to read-only bind mount should fail. stdout: {}, stderr: {}",
        ro_write_test.stdout,
        ro_write_test.stderr
    );
    println!("✓ RO bind mount correctly rejects writes");

    // Cleanup domain
    cleanup_domain(&domain_name);

    println!("✓ Bind mounts test passed");
    Ok(())
}
