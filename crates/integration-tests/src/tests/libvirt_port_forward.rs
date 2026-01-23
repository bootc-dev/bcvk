//! Integration tests for libvirt port forwarding functionality
//!
//! These tests verify:
//! - Port forwarding argument parsing and validation
//! - QEMU netdev configuration with hostfwd
//! - Actual network connectivity through forwarded ports

use color_eyre::Result;
use integration_tests::integration_test;
use xshell::cmd;

use crate::{get_bck_command, get_test_image, shell, LIBVIRT_INTEGRATION_TEST_LABEL};

/// Test port forwarding argument parsing
fn test_libvirt_port_forward_parsing() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;

    // Test valid port forwarding formats
    let valid_port_tests: Vec<&[&str]> = vec![
        &["--port", "8080:80"],
        &["--port", "80:80"],
        &["--port", "3000:3000", "--port", "8080:80"],
        &["-p", "9090:90"],
    ];

    for ports in valid_port_tests {
        let output = cmd!(sh, "{bck} libvirt run {ports...} --help")
            .ignore_status()
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            assert!(
                !stderr.contains("invalid") && !stderr.contains("parse"),
                "Port forwarding options should be parsed correctly: {:?}, stderr: {}",
                ports,
                stderr
            );
        } else {
            assert!(
                stdout.contains("Usage") || stdout.contains("USAGE"),
                "Should show help output when using --help"
            );
        }
    }

    println!("✓ Port forwarding argument parsing validated");
    Ok(())
}
integration_test!(test_libvirt_port_forward_parsing);

/// Test port forwarding error handling for invalid formats
fn test_libvirt_port_forward_invalid() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let test_image = get_test_image();

    // Generate unique domain name for this test
    let domain_name = format!(
        "test-invalid-port-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    // Test invalid port forwarding formats (should fail)
    let invalid_port_tests: Vec<(&[&str], &str)> = vec![
        (&["--port", "8080"], "missing guest port"),
        (&["--port", "abc:80"], "invalid host port"),
        (&["--port", "8080:xyz"], "invalid guest port"),
        (&["--port", "70000:80"], "port out of range"),
    ];

    for (ports, error_desc) in invalid_port_tests {
        let output = cmd!(
            sh,
            "{bck} libvirt run --name {domain_name} --transient --filesystem ext4 {ports...} {test_image}"
        )
        .ignore_status()
        .output()?;

        // Cleanup in case domain was partially created
        cleanup_domain(&domain_name);

        assert!(
            !output.status.success(),
            "Should fail for invalid port format: {}",
            error_desc
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("port")
                || stderr.contains("invalid")
                || stderr.contains("parse")
                || stderr.contains("Invalid"),
            "Should have error message about port format for case: {}, stderr: {}",
            error_desc,
            stderr
        );
    }

    println!("✓ Port forwarding error handling validated");
    Ok(())
}
integration_test!(test_libvirt_port_forward_invalid);

/// Test that port forwarding is correctly configured in domain XML
fn test_libvirt_port_forward_xml() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let test_image = get_test_image();
    let label = LIBVIRT_INTEGRATION_TEST_LABEL;

    // Generate unique domain name for this test
    let domain_name = format!(
        "test-port-xml-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    println!(
        "Testing port forwarding XML configuration with domain: {}",
        domain_name
    );

    // Cleanup any existing domain with this name
    cleanup_domain(&domain_name);

    // Create domain with port forwarding
    println!("Creating libvirt domain with port forwarding...");
    let create_output = cmd!(
        sh,
        "{bck} libvirt run --name {domain_name} --label {label} --port 8080:80 --port 9090:8080 --filesystem ext4 {test_image}"
    )
    .ignore_status()
    .output()?;

    let create_stdout = String::from_utf8_lossy(&create_output.stdout);
    let create_stderr = String::from_utf8_lossy(&create_output.stderr);

    println!("Create stdout: {}", create_stdout);
    println!("Create stderr: {}", create_stderr);

    if !create_output.status.success() {
        cleanup_domain(&domain_name);
        panic!(
            "Failed to create domain with port forwarding: {}",
            create_stderr
        );
    }

    println!("Successfully created domain: {}", domain_name);

    // Verify port forwarding in output
    assert!(
        create_stdout.contains("Port forwarding:")
            || create_stdout.contains("localhost:8080")
            || create_stdout.contains("localhost:9090"),
        "Output should mention port forwarding configuration"
    );

    // Check domain XML for QEMU args with port forwarding
    println!("Checking domain XML for port forwarding configuration...");
    let sh = shell()?;
    let domain_xml = cmd!(sh, "virsh dumpxml {domain_name}").read()?;

    // Verify QEMU commandline arguments contain hostfwd
    assert!(
        domain_xml.contains("qemu:commandline") || domain_xml.contains("qemu:arg"),
        "Domain XML should contain QEMU commandline namespace"
    );

    assert!(
        domain_xml.contains("hostfwd"),
        "Domain XML should contain hostfwd configuration for port forwarding"
    );

    // Verify specific port forwards are present
    assert!(
        domain_xml.contains("8080") && domain_xml.contains("80"),
        "Domain XML should contain port forwarding 8080:80"
    );

    assert!(
        domain_xml.contains("9090") && domain_xml.contains("8080"),
        "Domain XML should contain port forwarding 9090:8080"
    );

    println!("✓ Domain XML contains expected port forwarding configuration");

    // Cleanup domain
    cleanup_domain(&domain_name);

    println!("✓ Port forwarding XML configuration test passed");
    Ok(())
}
integration_test!(test_libvirt_port_forward_xml);

/// Test actual network connectivity through forwarded ports
fn test_libvirt_port_forward_connectivity() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let test_image = get_test_image();
    let label = LIBVIRT_INTEGRATION_TEST_LABEL;

    // Find an available port on the host
    let host_port = find_available_port()?;
    let port_mapping = format!("{}:8080", host_port);

    // Generate unique domain name for this test
    let domain_name = format!(
        "test-port-conn-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    println!(
        "Testing port forwarding connectivity with domain: {} on port {}",
        domain_name, host_port
    );

    // Cleanup any existing domain with this name
    cleanup_domain(&domain_name);

    // Create domain with port forwarding (forward host_port to guest port 8080)
    println!(
        "Creating libvirt domain with port forwarding {}:8080...",
        host_port
    );
    let create_output = cmd!(
        sh,
        "{bck} libvirt run --name {domain_name} --label {label} --port {port_mapping} --filesystem ext4 --ssh-wait {test_image}"
    )
    .ignore_status()
    .output()?;

    let create_stdout = String::from_utf8_lossy(&create_output.stdout);
    let create_stderr = String::from_utf8_lossy(&create_output.stderr);

    println!("Create stdout: {}", create_stdout);
    println!("Create stderr: {}", create_stderr);

    if !create_output.status.success() {
        cleanup_domain(&domain_name);
        panic!(
            "Failed to create domain with port forwarding: {}",
            create_stderr
        );
    }

    println!("Successfully created domain: {}", domain_name);

    // SSH is already available due to --ssh-wait flag
    println!("✓ SSH is ready (via --ssh-wait)");

    // Start a simple HTTP server on port 8080 inside the VM using Python
    println!("Starting HTTP server on port 8080 inside VM...");

    // Create a test file to serve
    println!("Creating test file in VM...");
    let create_file_cmd = "echo 'port-forward-test-success' > /tmp/test.txt";
    let create_file_output = cmd!(
        sh,
        "{bck} libvirt ssh --timeout 10 {domain_name} -- sh -c {create_file_cmd}"
    )
    .ignore_status()
    .output()?;

    if !create_file_output.status.success() {
        let stderr = String::from_utf8_lossy(&create_file_output.stderr);
        cleanup_domain(&domain_name);
        panic!("Failed to create test file in VM: {}", stderr);
    }
    println!("✓ Test file created successfully");

    // Start HTTP server in background
    // Use a combination of techniques to ensure the process fully detaches:
    // 1. Redirect stdin from /dev/null
    // 2. Redirect stdout and stderr to a file
    // 3. Put the command in a subshell and background it
    // 4. Use 'exec' to replace the shell with the server, making it cleaner
    println!("Starting background HTTP server...");
    let start_server_cmd =
        "(cd /tmp && exec python3 -m http.server 8080 > /tmp/http.log 2>&1 < /dev/null &) && sleep 0.1";
    let start_server_output = cmd!(
        sh,
        "{bck} libvirt ssh --timeout 10 {domain_name} -- bash -c {start_server_cmd}"
    )
    .ignore_status()
    .output()?;

    if !start_server_output.status.success() {
        let stderr = String::from_utf8_lossy(&start_server_output.stderr);
        cleanup_domain(&domain_name);
        panic!("Failed to start HTTP server in VM: {}", stderr);
    }
    println!("✓ HTTP server command executed");

    // Wait a bit for the server to start
    println!("Waiting for HTTP server to start...");
    std::thread::sleep(std::time::Duration::from_secs(5));

    // Test connectivity from host to forwarded port using curl
    println!("Testing connectivity to forwarded port from host...");
    let sh = shell()?;
    let mut retry_count = 0;
    let max_retries = 5;
    let mut connection_success = false;
    let url = format!("http://localhost:{}/test.txt", host_port);

    while retry_count < max_retries {
        match cmd!(sh, "curl -s -m 5 {url}").ignore_status().output() {
            Ok(output) if output.status.success() => {
                let response = String::from_utf8_lossy(&output.stdout);
                println!("Received response: {}", response);

                if response.contains("port-forward-test-success") {
                    println!(
                        "✓ Successfully connected to forwarded port and received expected content"
                    );
                    connection_success = true;
                    break;
                }
            }
            Ok(output) => {
                println!(
                    "Attempt {}/{}: Connection failed, retrying... stderr: {}",
                    retry_count + 1,
                    max_retries,
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(e) => {
                println!(
                    "Attempt {}/{}: curl error: {}",
                    retry_count + 1,
                    max_retries,
                    e
                );
            }
        }

        retry_count += 1;
        if retry_count < max_retries {
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    }

    // Cleanup domain before assertions
    cleanup_domain(&domain_name);

    assert!(
        connection_success,
        "Failed to connect to forwarded port after {} attempts",
        max_retries
    );

    println!("✓ Port forwarding connectivity test passed");
    Ok(())
}
integration_test!(test_libvirt_port_forward_connectivity);

/// Helper function to cleanup domain
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

/// Find an available port on the host
fn find_available_port() -> Result<u16> {
    use std::net::TcpListener;

    // Try to bind to port 0, which will allocate an available port
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();

    // Drop the listener to free the port
    drop(listener);

    Ok(port)
}
