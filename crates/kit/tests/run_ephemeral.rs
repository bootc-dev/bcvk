use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[test]
#[ignore] // This test requires QEMU, virtiofsd and a bootc container image
fn test_run_ephemeral_poweroff() {
    // Use systemd.unit=poweroff.target to boot and immediately shut down
    let test_image = "quay.io/fedora/fedora-bootc:42";

    // Build the bootc-kit binary first
    let build_status = Command::new("cargo")
        .args(["build", "--release"])
        .status()
        .expect("Failed to build bootc-kit");

    assert!(build_status.success(), "Failed to build bootc-kit");

    // Run with poweroff target - should boot and shut down cleanly
    let mut child = Command::new("../../target/release/bootc-kit")
        .args([
            "run-ephemeral",
            test_image,
            "--memory",
            "512",
            "--vcpus",
            "1",
            "--kernel-args",
            "systemd.unit=poweroff.target",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start bootc-kit run-ephemeral");

    // Give it more time for the full boot process
    let timeout = Duration::from_secs(120);
    let start = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Process has exited - this is expected with poweroff.target
                println!("VM exited with status: {:?}", status);

                // Check output for any obvious errors
                let output = child
                    .wait_with_output()
                    .unwrap_or_else(|_| std::process::Output {
                        status,
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                    });

                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);

                println!("stdout: {}", stdout);
                println!("stderr: {}", stderr);

                // For poweroff.target, we expect the process to exit
                // The important thing is that it doesn't crash before trying to boot
                assert!(
                    !stderr.contains("No kernel found"),
                    "Kernel extraction should work"
                );
                assert!(
                    !stderr.contains("No initrd found"),
                    "Initrd extraction should work"
                );

                break;
            }
            Ok(None) => {
                // Still running
                if start.elapsed() > timeout {
                    child.kill().ok();
                    panic!("Test timed out after {:?}", timeout);
                }
                thread::sleep(Duration::from_millis(500));
            }
            Err(e) => {
                panic!("Error waiting for child process: {}", e);
            }
        }
    }
}

#[test]
#[ignore] // This test requires QEMU, KVM, virtiofsd and a bootc container image
fn test_run_ephemeral_basic() {
    // Use /bin/true as init for immediate exit
    let test_image = "quay.io/fedora/fedora-bootc:42";

    // Build the bootc-kit binary first
    let build_status = Command::new("cargo")
        .args(["build", "--release"])
        .status()
        .expect("Failed to build bootc-kit");

    assert!(build_status.success(), "Failed to build bootc-kit");

    // Prepare the command with a short-lived init that will exit quickly
    let mut child = Command::new("../../target/release/bootc-kit")
        .args([
            "run-ephemeral",
            test_image,
            "--init",
            "/bin/true", // Use /bin/true to exit immediately
            "--memory",
            "512",
            "--vcpus",
            "1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start bootc-kit run-ephemeral");

    // Give it some time to start up and then exit
    let timeout = Duration::from_secs(60);
    let start = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Process has exited
                println!("VM exited with status: {:?}", status);
                break;
            }
            Ok(None) => {
                // Still running
                if start.elapsed() > timeout {
                    child.kill().ok();
                    panic!("Test timed out after {:?}", timeout);
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                panic!("Error waiting for child process: {}", e);
            }
        }
    }
}

#[test]
fn test_run_ephemeral_help() {
    let output = Command::new("cargo")
        .args(["run", "--", "run-ephemeral", "--help"])
        .output()
        .expect("Failed to run help command");

    assert!(output.status.success(), "Help command failed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Run a container image as an ephemeral VM"));
    assert!(stdout.contains("--init"));
    assert!(stdout.contains("--memory"));
    assert!(stdout.contains("--vcpus"));
}

#[test]
fn test_run_ephemeral_missing_image() {
    let output = Command::new("cargo")
        .args(["run", "--", "run-ephemeral"])
        .output()
        .expect("Failed to run command");

    assert!(
        !output.status.success(),
        "Command should fail without image argument"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("required") || stderr.contains("IMAGE"),
        "Error message should mention missing image"
    );
}
