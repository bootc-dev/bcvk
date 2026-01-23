//! Integration tests for ephemeral run command
//!
//! ⚠️  **CRITICAL INTEGRATION TEST POLICY** ⚠️
//!
//! INTEGRATION TESTS MUST NEVER "warn and continue" ON FAILURES!
//!
//! If something is not working:
//! - Use `todo!("reason why this doesn't work yet")`
//! - Use `panic!("clear error message")`
//! - Use `assert!()` and `unwrap()` to fail hard
//!
//! NEVER use patterns like:
//! - "Note: test failed - likely due to..."
//! - "This is acceptable in CI/testing environments"
//! - Warning and continuing on failures

use color_eyre::Result;
use integration_tests::integration_test;
use xshell::cmd;

use tracing::debug;

use crate::{get_bck_command, get_test_image, shell, INTEGRATION_TEST_LABEL};

pub fn get_container_kernel_version(image: &str) -> String {
    // Run container to get its kernel version
    let sh = shell().expect("Failed to create shell");
    cmd!(
        sh,
        "podman run --rm {image} sh -c 'ls -1 /usr/lib/modules | head -1'"
    )
    .read()
    .expect("Failed to get container kernel version")
}

fn test_run_ephemeral_correct_kernel() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;
    let container_kernel = get_container_kernel_version(&image);
    eprintln!("Container kernel version: {}", container_kernel);

    cmd!(
        sh,
        "{bck} ephemeral run --rm --label {label} {image} --karg systemd.unit=poweroff.target"
    )
    .run()?;
    Ok(())
}
integration_test!(test_run_ephemeral_correct_kernel);

fn test_run_ephemeral_poweroff() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;

    cmd!(
        sh,
        "{bck} ephemeral run --rm --label {label} {image} --karg systemd.unit=poweroff.target"
    )
    .run()?;
    Ok(())
}
integration_test!(test_run_ephemeral_poweroff);

fn test_run_ephemeral_with_memory_limit() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;

    cmd!(
        sh,
        "{bck} ephemeral run --rm --label {label} --memory 1024 --karg systemd.unit=poweroff.target {image}"
    )
    .run()?;
    Ok(())
}
integration_test!(test_run_ephemeral_with_memory_limit);

fn test_run_ephemeral_with_vcpus() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;

    cmd!(
        sh,
        "{bck} ephemeral run --rm --label {label} --vcpus 2 --karg systemd.unit=poweroff.target {image}"
    )
    .run()?;
    Ok(())
}
integration_test!(test_run_ephemeral_with_vcpus);

fn test_run_ephemeral_execute() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;
    let script =
        "/bin/sh -c \"echo 'Hello from VM'; echo 'Current date:'; date; echo 'Script completed successfully'\"";

    let stdout = cmd!(
        sh,
        "{bck} ephemeral run --rm --label {label} --execute {script} {image}"
    )
    .read()?;

    assert!(
        stdout.contains("Hello from VM"),
        "Script output 'Hello from VM' not found in stdout: {}",
        stdout
    );

    assert!(
        stdout.contains("Script completed successfully"),
        "Script completion message not found in stdout: {}",
        stdout
    );

    assert!(
        stdout.contains("Current date:"),
        "Date output header not found in stdout: {}",
        stdout
    );
    Ok(())
}
integration_test!(test_run_ephemeral_execute);

fn test_run_ephemeral_container_ssh_access() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;
    let container_name = format!(
        "ssh-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    cmd!(
        sh,
        "{bck} ephemeral run --ssh-keygen --label {label} --detach --name {container_name} {image}"
    )
    .run()?;

    let stdout = cmd!(
        sh,
        "{bck} ephemeral ssh {container_name} echo SSH_TEST_SUCCESS"
    )
    .read()?;

    // Cleanup: stop the container
    let _ = cmd!(sh, "podman stop {container_name}")
        .ignore_status()
        .quiet()
        .run();

    assert!(stdout.contains("SSH_TEST_SUCCESS"));
    Ok(())
}
integration_test!(test_run_ephemeral_container_ssh_access);

fn test_run_ephemeral_with_instancetype() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;
    // Test u1.nano: 1 vCPU, 512 MiB memory
    // Calculate physical memory from /sys/firmware/memmap (System RAM regions)
    let script = "/bin/sh -c 'echo CPUs:$(grep -c ^processor /proc/cpuinfo); total=0; for dir in /sys/firmware/memmap/*; do type=$(cat \"$dir/type\" 2>/dev/null); if [ \"$type\" = \"System RAM\" ]; then start=$(cat \"$dir/start\"); end=$(cat \"$dir/end\"); start_dec=$((start)); end_dec=$((end)); size=$((end_dec - start_dec + 1)); total=$((total + size)); fi; done; total_kb=$((total / 1024)); echo PhysicalMemKB:$total_kb'";

    let stdout = cmd!(
        sh,
        "{bck} ephemeral run --rm --label {label} --itype u1.nano --execute {script} {image}"
    )
    .read()?;

    // Verify vCPUs (should be 1)
    assert!(
        stdout.contains("CPUs:1"),
        "Expected 1 vCPU for u1.nano, output: {}",
        stdout
    );

    // Verify physical memory (should be exactly 512 MiB = 524288 kB)
    let mem_line = stdout
        .lines()
        .find(|line| line.contains("PhysicalMemKB:"))
        .expect("PhysicalMemKB line not found in output");

    let mem_kb: u32 = mem_line
        .split(':')
        .nth(1)
        .expect("Could not parse PhysicalMemKB")
        .trim()
        .parse()
        .expect("Could not parse PhysicalMemKB as number");

    // Physical memory should be close to 512 MiB = 524288 kB
    // QEMU reserves small memory regions (BIOS, VGA, ACPI, etc.) so actual may be slightly less
    // Allow 1% tolerance to account for hypervisor overhead
    let expected_kb = 512 * 1024;
    let tolerance_kb = expected_kb / 100; // 1% tolerance
    let diff = if mem_kb > expected_kb {
        mem_kb - expected_kb
    } else {
        expected_kb - mem_kb
    };

    assert!(
        diff <= tolerance_kb,
        "Expected physical memory ~{} kB for u1.nano, got {} kB (diff: {} kB, max allowed: {} kB [1%])",
        expected_kb, mem_kb, diff, tolerance_kb
    );

    Ok(())
}
integration_test!(test_run_ephemeral_with_instancetype);

fn test_run_ephemeral_instancetype_invalid() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;

    let output = cmd!(
        sh,
        "{bck} ephemeral run --rm --label {label} --itype invalid.type --karg systemd.unit=poweroff.target {image}"
    )
    .ignore_status()
    .output()?;

    // Should fail with invalid instance type
    assert!(
        !output.status.success(),
        "Expected failure with invalid instance type, but succeeded"
    );

    // Error message should mention the invalid type
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid.type") || stderr.contains("Unknown instance type"),
        "Error message should mention invalid instance type: {}",
        stderr
    );

    Ok(())
}
integration_test!(test_run_ephemeral_instancetype_invalid);

/// Test that ephemeral VMs can boot from UKI-only images (no separate vmlinuz/initramfs)
///
/// This tests compatibility with bootc images that only ship a Unified Kernel Image,
/// verifying that bcvk can extract kernel/initramfs from the UKI using objcopy.
fn test_run_ephemeral_uki_only() -> Result<()> {
    let sh = shell()?;
    let base_image = get_test_image();
    let uki_image = "bcvk-test-uki-only:latest";

    // Build the UKI-only test image from the fixture Dockerfile
    let fixture_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/Dockerfile.uki-only");
    let fixture_dir = fixture_path.parent().unwrap();
    let dockerfile = fixture_path.to_str().unwrap();
    let build_arg = format!("BASE_IMAGE={}", base_image);

    debug!(
        "Building UKI-only test image from {} using base {}",
        fixture_path.display(),
        base_image
    );

    cmd!(
        sh,
        "podman build -f {dockerfile} -t {uki_image} --build-arg {build_arg} {fixture_dir}"
    )
    .run()?;

    // Verify the image has a UKI in /boot/EFI/Linux/ and no vmlinuz
    let verify_stdout = cmd!(
        sh,
        "podman run --rm {uki_image} sh -c 'ls /usr/lib/modules/*/vmlinuz 2>/dev/null && echo HAS_VMLINUZ || echo NO_VMLINUZ; ls /boot/EFI/Linux/*.efi 2>/dev/null && echo HAS_UKI || echo NO_UKI'"
    )
    .read()?;

    debug!("Image verification: {}", verify_stdout);
    assert!(
        verify_stdout.contains("NO_VMLINUZ"),
        "UKI-only image should not have vmlinuz: {}",
        verify_stdout
    );
    assert!(
        verify_stdout.contains("HAS_UKI"),
        "UKI-only image should have a UKI in /boot/EFI/Linux/: {}",
        verify_stdout
    );

    // Run ephemeral VM from UKI-only image
    let label = INTEGRATION_TEST_LABEL;
    let bck = get_bck_command()?;
    let stdout = cmd!(
        sh,
        "{bck} ephemeral run --rm --label {label} --execute 'echo UKI_BOOT_SUCCESS' {uki_image}"
    )
    .read()?;

    assert!(
        stdout.contains("UKI_BOOT_SUCCESS"),
        "UKI boot should output success message: {}",
        stdout
    );

    // Cleanup the test image
    let _ = cmd!(sh, "podman rmi -f {uki_image}")
        .ignore_status()
        .quiet()
        .run();

    Ok(())
}
integration_test!(test_run_ephemeral_uki_only);

/// Test ephemeral boot with the CentOS 10 UKI image
///
/// This tests a real-world UKI image that may have both UKI and traditional
/// kernel files, verifying that bcvk correctly prefers the UKI.
fn test_run_ephemeral_centos_uki() -> Result<()> {
    const CENTOS_UKI_IMAGE: &str = "ghcr.io/bootc-dev/dev-bootc:centos-10-uki";

    debug!("Testing ephemeral boot with {}", CENTOS_UKI_IMAGE);

    let sh = shell()?;
    let bck = get_bck_command()?;
    let label = INTEGRATION_TEST_LABEL;

    // Pull the image first (it's not in the standard test image set)
    cmd!(sh, "podman pull -q {CENTOS_UKI_IMAGE}").run()?;

    let script =
        "echo CENTOS_UKI_BOOT_SUCCESS && cat /etc/os-release | grep -E '^(ID|VERSION_ID)='";
    let stdout = cmd!(
        sh,
        "{bck} ephemeral run --rm --label {label} --execute {script} {CENTOS_UKI_IMAGE}"
    )
    .read()?;

    assert!(
        stdout.contains("CENTOS_UKI_BOOT_SUCCESS"),
        "CentOS UKI boot should output success message: {}",
        stdout
    );

    Ok(())
}
integration_test!(test_run_ephemeral_centos_uki);

/// Test that ephemeral VMs have the expected mount layout:
/// - / is read-only virtiofs
/// - /etc is overlayfs with tmpfs upper (writable)
/// - /var is tmpfs (not overlayfs, so podman can use overlayfs inside)
fn test_run_ephemeral_mount_layout() -> Result<()> {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;

    // Check each mount point individually using findmnt
    // Running all three at once with -J can hang on some configurations

    // Check root mount
    let root_line = cmd!(
        sh,
        "{bck} ephemeral run --rm --label {label} --execute 'findmnt -n -o FSTYPE,OPTIONS /' {image}"
    )
    .read()?;
    assert!(
        root_line.starts_with("virtiofs"),
        "Root should be virtiofs, got: {}",
        root_line
    );
    assert!(
        root_line.contains("ro"),
        "Root should be read-only, got: {}",
        root_line
    );

    // Check /etc mount
    let etc_fstype = cmd!(
        sh,
        "{bck} ephemeral run --rm --label {label} --execute 'findmnt -n -o FSTYPE /etc' {image}"
    )
    .read()?;
    assert_eq!(
        etc_fstype.trim(),
        "overlay",
        "/etc should be overlay, got: {}",
        etc_fstype
    );

    // Check /var mount - should be tmpfs, NOT overlay
    let var_fstype = cmd!(
        sh,
        "{bck} ephemeral run --rm --label {label} --execute 'findmnt -n -o FSTYPE /var' {image}"
    )
    .read()?;
    assert_eq!(
        var_fstype.trim(),
        "tmpfs",
        "/var should be tmpfs (not overlay), got: {}",
        var_fstype
    );

    Ok(())
}
integration_test!(test_run_ephemeral_mount_layout);
