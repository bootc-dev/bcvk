//! Integration tests for `bcvk libvirt run-anaconda` command
//!
//! These tests verify the anaconda-based libvirt VM creation workflow:
//! - Creating VMs using anaconda with kickstart files
//! - SSH connectivity after VM creation
//! - All the same lifecycle management as `bcvk libvirt run`
//!
//! **PREREQUISITES:**
//! - The anaconda-bootc container must be built first:
//!   `podman build -t localhost/anaconda-bootc:latest containers/anaconda-bootc/`
//! - A bootc image must be available in local container storage
//!
//! **NOTE:** These tests are skipped if the anaconda container is not available.

use color_eyre::Result;
use integration_tests::integration_test;
use scopeguard::defer;
use xshell::cmd;

use crate::{get_bck_command, get_test_image, shell, LIBVIRT_INTEGRATION_TEST_LABEL};

const ANACONDA_IMAGE: &str = "localhost/anaconda-bootc:latest";

/// Check if the anaconda container image is available
fn anaconda_image_available() -> bool {
    let sh = match shell() {
        Ok(sh) => sh,
        Err(_) => return false,
    };
    cmd!(sh, "podman image exists {ANACONDA_IMAGE}")
        .quiet()
        .run()
        .is_ok()
}

/// Generate a random alphanumeric suffix for VM names to avoid collisions
fn random_suffix() -> String {
    use rand::{distr::Alphanumeric, Rng};
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(8)
        .map(char::from)
        .collect()
}

/// Create a kickstart file for testing
fn create_test_kickstart(dir: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let ks_path = dir.join("test.ks");
    let ks_content = r#"# Test kickstart for bcvk libvirt run-anaconda integration tests
text
lang en_US.UTF-8
keyboard us
timezone UTC --utc
network --bootproto=dhcp --activate

# Target only the output disk, ignore the swap disk
ignoredisk --only-use=/dev/disk/by-id/virtio-output

zerombr
clearpart --all --initlabel

# Let anaconda create required boot partitions
reqpart --add-boot

# Root partition
part / --fstype=xfs --grow

rootpw --lock

poweroff
"#;
    std::fs::write(&ks_path, ks_content)?;
    Ok(ks_path)
}

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

/// Test basic `bcvk libvirt run-anaconda` functionality
///
/// This test:
/// 1. Creates a VM using anaconda with a kickstart file
/// 2. Waits for SSH to be available
/// 3. Verifies the VM is running
/// 4. Cleans up
fn test_libvirt_run_anaconda_basic() -> Result<()> {
    if !anaconda_image_available() {
        eprintln!(
            "Skipping test_libvirt_run_anaconda_basic: {} not available",
            ANACONDA_IMAGE
        );
        eprintln!(
            "Build it with: podman build -t {} containers/anaconda-bootc/",
            ANACONDA_IMAGE
        );
        return Ok(());
    }

    let sh = shell()?;
    let bck = get_bck_command()?;
    let test_image = get_test_image();
    let label = LIBVIRT_INTEGRATION_TEST_LABEL;

    // Generate unique domain name for this test
    let domain_name = format!("test-run-anaconda-{}", random_suffix());

    println!(
        "Testing bcvk libvirt run-anaconda with domain: {}",
        domain_name
    );

    // Create temporary kickstart file
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp directory");
    let ks_path = create_test_kickstart(temp_dir.path()).expect("Failed to create kickstart");
    let ks_path_str = ks_path.to_string_lossy().into_owned();

    // Cleanup any existing domain with this name
    cleanup_domain(&domain_name);

    // Set up cleanup guard that will run on scope exit
    defer! {
        cleanup_domain(&domain_name);
    }

    // Create domain with anaconda, wait for SSH
    // Use BIOS firmware because the kickstart uses reqpart --add-boot which creates
    // BIOS boot partitions when anaconda runs in the ephemeral QEMU VM
    println!("Creating libvirt domain via anaconda...");
    cmd!(
        sh,
        "{bck} libvirt run-anaconda --name {domain_name} --label {label} --kickstart {ks_path_str} --firmware bios --ssh-wait {test_image}"
    )
    .run()?;

    println!("Successfully created domain: {}", domain_name);

    // Verify domain is running
    println!("Verifying domain is running...");
    let dominfo = cmd!(sh, "virsh dominfo {domain_name}").read()?;
    assert!(
        dominfo.contains("running") || dominfo.contains("idle"),
        "Domain should be running. dominfo: {}",
        dominfo
    );
    println!("Domain is running");

    // Verify we can SSH into the VM
    println!("Testing SSH connectivity...");
    let hostname_output = cmd!(sh, "{bck} libvirt ssh {domain_name} -- hostname").read()?;
    assert!(
        !hostname_output.is_empty(),
        "Should be able to get hostname via SSH"
    );
    println!(
        "SSH connectivity verified, hostname: {}",
        hostname_output.trim()
    );

    // Verify domain metadata contains anaconda install method
    println!("Checking domain metadata...");
    let domain_xml = cmd!(sh, "virsh dumpxml {domain_name}").read()?;
    assert!(
        domain_xml.contains("bootc:install-method") && domain_xml.contains("anaconda"),
        "Domain XML should contain anaconda install-method metadata"
    );
    println!("Domain metadata correctly shows anaconda install method");

    println!("libvirt run-anaconda basic test passed");
    Ok(())
}
integration_test!(test_libvirt_run_anaconda_basic);

/// Test `bcvk libvirt run-anaconda --replace` functionality
fn test_libvirt_run_anaconda_replace() -> Result<()> {
    if !anaconda_image_available() {
        eprintln!(
            "Skipping test_libvirt_run_anaconda_replace: {} not available",
            ANACONDA_IMAGE
        );
        return Ok(());
    }

    let sh = shell()?;
    let bck = get_bck_command()?;
    let test_image = get_test_image();
    let label = LIBVIRT_INTEGRATION_TEST_LABEL;

    let domain_name = format!("test-anaconda-replace-{}", random_suffix());

    println!(
        "Testing bcvk libvirt run-anaconda --replace with domain: {}",
        domain_name
    );

    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp directory");
    let ks_path = create_test_kickstart(temp_dir.path()).expect("Failed to create kickstart");
    let ks_path_str = ks_path.to_string_lossy().into_owned();

    cleanup_domain(&domain_name);

    defer! {
        cleanup_domain(&domain_name);
    }

    // Create initial domain
    // Use BIOS firmware because the kickstart creates BIOS boot partitions
    println!("Creating initial domain...");
    cmd!(
        sh,
        "{bck} libvirt run-anaconda --name {domain_name} --label {label} --kickstart {ks_path_str} --firmware bios {test_image}"
    )
    .run()?;
    println!("Initial domain created");

    // Replace the domain
    println!("Replacing domain with --replace...");
    cmd!(
        sh,
        "{bck} libvirt run-anaconda --name {domain_name} --label {label} --kickstart {ks_path_str} --firmware bios --replace {test_image}"
    )
    .run()?;
    println!("Domain replaced successfully");

    // Verify replaced domain is running
    let dominfo = cmd!(sh, "virsh dominfo {domain_name}").read()?;
    assert!(
        dominfo.contains("running") || dominfo.contains("idle"),
        "Replaced domain should be running"
    );
    println!("Replaced domain is running");

    println!("libvirt run-anaconda --replace test passed");
    Ok(())
}
integration_test!(test_libvirt_run_anaconda_replace);

/// Test `bcvk libvirt run-anaconda --transient` functionality
fn test_libvirt_run_anaconda_transient() -> Result<()> {
    if !anaconda_image_available() {
        eprintln!(
            "Skipping test_libvirt_run_anaconda_transient: {} not available",
            ANACONDA_IMAGE
        );
        return Ok(());
    }

    let sh = shell()?;
    let bck = get_bck_command()?;
    let test_image = get_test_image();
    let label = LIBVIRT_INTEGRATION_TEST_LABEL;

    let domain_name = format!("test-anaconda-transient-{}", random_suffix());

    println!(
        "Testing bcvk libvirt run-anaconda --transient with domain: {}",
        domain_name
    );

    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp directory");
    let ks_path = create_test_kickstart(temp_dir.path()).expect("Failed to create kickstart");
    let ks_path_str = ks_path.to_string_lossy().into_owned();

    cleanup_domain(&domain_name);

    defer! {
        cleanup_domain(&domain_name);
    }

    // Create transient domain
    // Use BIOS firmware because the kickstart creates BIOS boot partitions
    println!("Creating transient domain...");
    cmd!(
        sh,
        "{bck} libvirt run-anaconda --name {domain_name} --label {label} --kickstart {ks_path_str} --firmware bios --transient {test_image}"
    )
    .run()?;
    println!("Transient domain created");

    // Verify domain is transient
    let dominfo = cmd!(sh, "virsh dominfo {domain_name}").read()?;
    assert!(
        dominfo.contains("Persistent:") && dominfo.contains("no"),
        "Domain should be transient. dominfo: {}",
        dominfo
    );
    println!("Domain is correctly marked as transient");

    // Stop the domain (should disappear since it's transient)
    println!("Stopping transient domain...");
    cmd!(sh, "virsh destroy {domain_name}").run()?;

    // Poll for domain disappearance
    let start_time = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(10);
    let mut domain_disappeared = false;

    while start_time.elapsed() < timeout {
        let domain_list = cmd!(sh, "virsh list --all --name").ignore_status().read()?;
        if !domain_list.contains(&domain_name) {
            domain_disappeared = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    assert!(
        domain_disappeared,
        "Transient domain should disappear after shutdown"
    );
    println!("Transient domain correctly disappeared after shutdown");

    println!("libvirt run-anaconda --transient test passed");
    Ok(())
}
integration_test!(test_libvirt_run_anaconda_transient);
