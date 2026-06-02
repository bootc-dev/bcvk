//! Integration tests for ephemeral scp command
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

use integration_tests::integration_test;
use itest::TestResult;
use scopeguard::defer;
use xshell::cmd;

use std::fs;
use tempfile::TempDir;

use crate::{get_bck_command, get_test_image, shell, INTEGRATION_TEST_LABEL};

/// Test that ephemeral SCP validates syntax correctly
fn test_ephemeral_scp_syntax() -> TestResult {
    let sh = shell()?;
    let bck = get_bck_command()?;

    // Test that SCP requires at least one DOMAIN: prefix
    let output = cmd!(sh, "{bck} ephemeral scp test-container /local/a /local/b")
        .ignore_status()
        .output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "SCP with two local paths should fail"
    );
    assert!(
        stderr.contains("DOMAIN:"),
        "Error should mention DOMAIN: prefix: {}",
        stderr
    );

    // Test that SCP with two remote paths fails
    let output2 = cmd!(
        sh,
        "{bck} ephemeral scp test-container DOMAIN:/remote/a DOMAIN:/remote/b"
    )
    .ignore_status()
    .output()?;
    let stderr2 = String::from_utf8_lossy(&output2.stderr);

    assert!(
        !output2.status.success(),
        "SCP with two remote paths should fail"
    );
    assert!(
        stderr2.contains("DOMAIN:"),
        "Error should mention DOMAIN: prefix: {}",
        stderr2
    );

    println!("ephemeral SCP syntax tested");
    Ok(())
}
integration_test!(test_ephemeral_scp_syntax);

/// End-to-end SCP test: starts an ephemeral VM, copies files to and from it
fn test_ephemeral_scp_end_to_end() -> TestResult {
    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();
    let label = INTEGRATION_TEST_LABEL;
    let container_name = format!("test-scp-{}", std::process::id());

    println!("Starting ephemeral VM for SCP end-to-end test...");
    cmd!(
        sh,
        "{bck} ephemeral run --ssh-keygen --label {label} --detach --name {container_name} {image}"
    )
    .run()?;

    // Ensure the container is cleaned up even if the test fails
    defer! {
        let sh = shell().unwrap();
        let _ = cmd!(sh, "podman rm -f {container_name}")
            .ignore_status()
            .quiet()
            .run();
    }

    // Wait a little for SSH connectivity to be verified / ready
    println!("Waiting for SSH access to become ready...");
    let _ = cmd!(sh, "{bck} ephemeral ssh {container_name} echo READY").read()?;

    // --- Upload a file to the VM ---
    let upload_dir = TempDir::new()?;
    let upload_file = upload_dir.path().join("upload-test.txt");
    fs::write(&upload_file, "hello from host to ephemeral VM")?;
    let upload_path = upload_file.to_str().expect("non-UTF-8 temp path");

    println!("Uploading file to ephemeral VM...");
    cmd!(
        sh,
        "{bck} ephemeral scp {container_name} {upload_path} DOMAIN:/tmp/upload-test.txt"
    )
    .run()?;
    println!("✓ File uploaded");

    // Verify it arrived
    let cat_stdout = cmd!(
        sh,
        "{bck} ephemeral ssh {container_name} -- cat /tmp/upload-test.txt"
    )
    .read()?;
    assert_eq!(
        cat_stdout.trim(),
        "hello from host to ephemeral VM",
        "Uploaded file content mismatch"
    );
    println!("✓ Uploaded file content verified");

    // --- Download a file from the VM ---
    let download_dir = TempDir::new()?;
    let download_path = download_dir.path().join("os-release");
    let download_str = download_path.to_str().expect("non-UTF-8 temp path");

    // Get the expected os-release content first, and normalize CRLF to LF
    let expected_os_release = cmd!(
        sh,
        "{bck} ephemeral ssh {container_name} -- cat /etc/os-release"
    )
    .read()?
    .trim()
    .replace("\r", "");

    println!("Downloading /etc/os-release from ephemeral VM...");
    cmd!(
        sh,
        "{bck} ephemeral scp {container_name} DOMAIN:/etc/os-release {download_str}"
    )
    .run()?;

    let downloaded = fs::read_to_string(&download_path)?;
    assert_eq!(
        downloaded.trim(),
        expected_os_release,
        "Downloaded os-release mismatch"
    );
    println!(
        "✓ Downloaded /etc/os-release verified: {}",
        downloaded.trim()
    );

    // --- Recursive Copy to VM ---
    let local_rec_dir = TempDir::new()?;
    let file1 = local_rec_dir.path().join("file1.txt");
    let file2 = local_rec_dir.path().join("file2.txt");
    fs::write(&file1, "nested content 1")?;
    fs::write(&file2, "nested content 2")?;
    let local_rec_path = local_rec_dir.path().to_str().expect("non-UTF-8 path");

    println!("Recursive upload of directory to ephemeral VM...");
    cmd!(
        sh,
        "{bck} ephemeral scp {container_name} -r {local_rec_path} DOMAIN:/tmp/rec_upload"
    )
    .run()?;

    // Verify contents of the recursively uploaded directory
    let verify_rec_1 = cmd!(
        sh,
        "{bck} ephemeral ssh {container_name} -- cat /tmp/rec_upload/file1.txt"
    )
    .read()?;
    assert_eq!(verify_rec_1.trim(), "nested content 1");

    let verify_rec_2 = cmd!(
        sh,
        "{bck} ephemeral ssh {container_name} -- cat /tmp/rec_upload/file2.txt"
    )
    .read()?;
    assert_eq!(verify_rec_2.trim(), "nested content 2");
    println!("✓ Recursive upload verified successfully");

    // --- Recursive Download from VM ---
    let download_rec_dir = TempDir::new()?;
    let download_rec_path = download_rec_dir.path().join("downloaded_rec");
    let download_rec_str = download_rec_path.to_str().expect("non-UTF-8 path");

    println!("Recursive download of directory from ephemeral VM...");
    cmd!(
        sh,
        "{bck} ephemeral scp {container_name} -r DOMAIN:/tmp/rec_upload {download_rec_str}"
    )
    .run()?;

    assert!(
        download_rec_path.exists(),
        "Downloaded recursive directory should exist"
    );
    let downloaded_file1 = download_rec_path.join("file1.txt");
    let downloaded_file2 = download_rec_path.join("file2.txt");
    assert_eq!(
        fs::read_to_string(downloaded_file1)?.trim(),
        "nested content 1"
    );
    assert_eq!(
        fs::read_to_string(downloaded_file2)?.trim(),
        "nested content 2"
    );
    println!("✓ Recursive download verified successfully");

    println!("✓ Ephemeral SCP end-to-end test passed");
    Ok(())
}
integration_test!(test_ephemeral_scp_end_to_end);
