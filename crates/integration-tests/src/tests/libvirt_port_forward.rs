//! Integration tests for libvirt port forwarding functionality
//!
//! These tests verify:
//! - Port forwarding argument parsing and validation
//! - QEMU netdev configuration with hostfwd
//! - Actual network connectivity through forwarded ports

use color_eyre::Result;
use linkme::distributed_slice;
use std::process::Command;

use crate::{
    get_bck_command, get_test_image, run_bcvk, IntegrationTest, INTEGRATION_TESTS,
    LIBVIRT_INTEGRATION_TEST_LABEL,
};

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_PORT_FORWARD_PARSING: IntegrationTest = IntegrationTest::new(
    "test_libvirt_port_forward_parsing",
    test_libvirt_port_forward_parsing,
);

/// Test port forwarding argument parsing
fn test_libvirt_port_forward_parsing() -> Result<()> {
    let bck = get_bck_command()?;

    // Test valid port forwarding formats
    let valid_port_tests = vec![
        vec!["--port", "8080:80"],
        vec!["--port", "80:80"],
        vec!["--port", "3000:3000", "--port", "8080:80"],
        vec!["-p", "9090:90"],
    ];

    for ports in valid_port_tests {
        let mut args = vec!["libvirt", "run"];
        args.extend(ports.iter());
        args.push("--help"); // Just test parsing, don't actually run

        let output = Command::new(&bck)
            .args(&args)
            .output()
            .expect("Failed to run libvirt run with port forwarding");

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

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_PORT_FORWARD_INVALID: IntegrationTest = IntegrationTest::new(
    "test_libvirt_port_forward_invalid",
    test_libvirt_port_forward_invalid,
);

/// Test port forwarding error handling for invalid formats
fn test_libvirt_port_forward_invalid() -> Result<()> {
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
    let invalid_port_tests = vec![
        (vec!["--port", "8080"], "missing guest port"),
        (vec!["--port", "abc:80"], "invalid host port"),
        (vec!["--port", "8080:xyz"], "invalid guest port"),
        (vec!["--port", "70000:80"], "port out of range"),
    ];

    for (ports, error_desc) in invalid_port_tests {
        let mut args = vec![
            "libvirt",
            "run",
            "--name",
            &domain_name,
            "--transient",
            "--filesystem",
            "ext4",
        ];
        args.extend(ports.iter());
        args.push(&test_image);

        let output = Command::new(&bck).args(&args).output().expect(&format!(
            "Failed to run error case for port forwarding: {}",
            error_desc
        ));

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

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_PORT_FORWARD_XML: IntegrationTest = IntegrationTest::new(
    "test_libvirt_port_forward_xml",
    test_libvirt_port_forward_xml,
);

/// Test that port forwarding is correctly configured in domain XML
fn test_libvirt_port_forward_xml() -> Result<()> {
    let test_image = get_test_image();

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
    let create_output = run_bcvk(&[
        "libvirt",
        "run",
        "--name",
        &domain_name,
        "--label",
        LIBVIRT_INTEGRATION_TEST_LABEL,
        "--port",
        "8080:80",
        "--port",
        "9090:8080",
        "--filesystem",
        "ext4",
        &test_image,
    ])
    .expect("Failed to run libvirt run with port forwarding");

    println!("Create stdout: {}", create_output.stdout);
    println!("Create stderr: {}", create_output.stderr);

    if !create_output.success() {
        cleanup_domain(&domain_name);
        panic!(
            "Failed to create domain with port forwarding: {}",
            create_output.stderr
        );
    }

    println!("Successfully created domain: {}", domain_name);

    // Verify port forwarding in output
    assert!(
        create_output.stdout.contains("Port forwarding:")
            || create_output.stdout.contains("localhost:8080")
            || create_output.stdout.contains("localhost:9090"),
        "Output should mention port forwarding configuration"
    );

    // Check domain XML for QEMU args with port forwarding
    println!("Checking domain XML for port forwarding configuration...");
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

#[distributed_slice(INTEGRATION_TESTS)]
static TEST_LIBVIRT_PORT_FORWARD_CONNECTIVITY: IntegrationTest = IntegrationTest::new(
    "test_libvirt_port_forward_connectivity",
    test_libvirt_port_forward_connectivity,
);

/// Test actual network connectivity through forwarded ports
fn test_libvirt_port_forward_connectivity() -> Result<()> {
    let test_image = get_test_image();

    // Find an available port on the host
    let host_port = find_available_port()?;

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
    let create_output = run_bcvk(&[
        "libvirt",
        "run",
        "--name",
        &domain_name,
        "--label",
        LIBVIRT_INTEGRATION_TEST_LABEL,
        "--port",
        &format!("{}:8080", host_port),
        "--filesystem",
        "ext4",
        &test_image,
    ])
    .expect("Failed to run libvirt run with port forwarding");

    println!("Create stdout: {}", create_output.stdout);
    println!("Create stderr: {}", create_output.stderr);

    if !create_output.success() {
        cleanup_domain(&domain_name);
        panic!(
            "Failed to create domain with port forwarding: {}",
            create_output.stderr
        );
    }

    println!("Successfully created domain: {}", domain_name);

    // Wait for VM to boot and SSH to become available
    println!("Waiting for VM to boot and SSH to become available...");
    if let Err(e) = wait_for_ssh_available(&domain_name, 180) {
        cleanup_domain(&domain_name);
        panic!("Failed to establish SSH connection: {}", e);
    }

    // Start a simple HTTP server on port 8080 inside the VM using Python
    println!("Starting HTTP server on port 8080 inside VM...");

    // Create a test file to serve
    println!("Creating test file in VM...");
    let create_file = run_bcvk(&[
        "libvirt",
        "ssh",
        "--timeout",
        "10",
        &domain_name,
        "--",
        "sh",
        "-c",
        "echo 'port-forward-test-success' > /tmp/test.txt",
    ])
    .expect("Failed to create test file in VM");

    if !create_file.success() {
        cleanup_domain(&domain_name);
        panic!("Failed to create test file in VM: {}", create_file.stderr);
    }
    println!("✓ Test file created successfully");

    // Start HTTP server in background
    // Use a combination of techniques to ensure the process fully detaches:
    // 1. Redirect stdin from /dev/null
    // 2. Redirect stdout and stderr to a file
    // 3. Put the command in a subshell and background it
    // 4. Use 'exec' to replace the shell with the server, making it cleaner
    println!("Starting background HTTP server...");
    let start_server = run_bcvk(&[
        "libvirt",
        "ssh",
        "--timeout",
        "10",
        &domain_name,
        "--",
        "bash",
        "-c",
        "(cd /tmp && exec python3 -m http.server 8080 > /tmp/http.log 2>&1 < /dev/null &) && sleep 0.1",
    ])
    .expect("Failed to start HTTP server in VM");

    if !start_server.success() {
        cleanup_domain(&domain_name);
        panic!("Failed to start HTTP server in VM: {}", start_server.stderr);
    }
    println!("✓ HTTP server command executed");

    // Wait a bit for the server to start
    println!("Waiting for HTTP server to start...");
    std::thread::sleep(std::time::Duration::from_secs(5));

    // Test connectivity from host to forwarded port using curl
    println!("Testing connectivity to forwarded port from host...");
    let mut retry_count = 0;
    let max_retries = 5;
    let mut connection_success = false;

    while retry_count < max_retries {
        let curl_output = Command::new("curl")
            .args(&[
                "-s",
                "-m",
                "5", // 5 second timeout
                &format!("http://localhost:{}/test.txt", host_port),
            ])
            .output();

        match curl_output {
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
        // Check if we've exceeded the timeout before attempting SSH
        if start_time.elapsed() >= timeout_duration {
            return Err(format!("Timeout waiting for SSH after {} seconds", timeout_secs).into());
        }

        // Try a simple SSH command to test connectivity with a short timeout (5 seconds)
        // This prevents each SSH attempt from hanging for the default 30 seconds
        let ssh_test = run_bcvk(&[
            "libvirt",
            "ssh",
            "--timeout",
            "5",
            domain_name,
            "--",
            "echo",
            "ssh-ready",
        ]);

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

        // Wait 2 seconds before next attempt (since we already waited 5 seconds for SSH timeout)
        std::thread::sleep(std::time::Duration::from_secs(2));
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
