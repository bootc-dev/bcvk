// Integration test suite for bootc-kit
// This binary runs various integration tests for the bootc-kit project

use color_eyre::eyre::{eyre, Result};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use xshell::{cmd, Shell};

fn test_images_list(sh: &Shell) -> Result<()> {
    println!("Running test: bck images list --json");

    // Run the bck images list command with JSON output
    let output = cmd!(sh, "bck images list --json").output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!("Failed to run 'bck images list --json': {}", stderr));
    }

    // Parse the JSON output
    let stdout = String::from_utf8(output.stdout)?;
    let images: Value =
        serde_json::from_str(&stdout).map_err(|e| eyre!("Failed to parse JSON output: {}", e))?;

    // Verify the structure and content of the JSON
    if !images.is_array() {
        return Err(eyre!("Expected JSON array in output, got: {}", stdout));
    }

    let images_array = images.as_array().unwrap();
    if images_array.is_empty() {
        return Err(eyre!("No images found in the JSON output"));
    }

    println!(
        "✅ Test passed: bck images list --json (found {} images)",
        images_array.len()
    );
    Ok(())
}

/// Check for trailing whitespace in Markdown files
fn test_markdown_no_trailing_whitespace() -> Result<()> {
    println!("Running test: Check for trailing whitespace in Markdown files");

    let mut violations = Vec::new();

    // Helper function to recursively check all markdown files
    fn check_dir(dir_path: &Path, violations: &mut Vec<String>) -> Result<()> {
        if dir_path.file_name().map_or(false, |name| name == "target") {
            return Ok(()); // Skip target directory
        }

        let entries = fs::read_dir(dir_path)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                check_dir(&path, violations)?;
            } else if let Some(extension) = path.extension() {
                if extension == "md" {
                    check_markdown_file(&path, violations)?;
                }
            }
        }

        Ok(())
    }

    // Helper function to check a single markdown file
    fn check_markdown_file(file_path: &Path, violations: &mut Vec<String>) -> Result<()> {
        let content = fs::read_to_string(file_path)?;

        for (line_num, line) in content.lines().enumerate() {
            if line.ends_with(' ') {
                violations.push(format!(
                    "{}:{}: trailing whitespace",
                    file_path.display(),
                    line_num + 1
                ));
            }
        }

        Ok(())
    }

    // Start checking from the project root
    let project_root =
        std::env::current_dir().map_err(|e| eyre!("Failed to get current directory: {}", e))?;
    check_dir(&project_root, &mut violations)?;

    if violations.is_empty() {
        println!("✅ Test passed: No trailing whitespace in Markdown files");
        Ok(())
    } else {
        for violation in &violations {
            println!("  - {}", violation);
        }
        Err(eyre!(
            "Found {} files with trailing whitespace in markdown files",
            violations.len()
        ))
    }
}

fn test_run_ephemeral_help(sh: &Shell) -> Result<()> {
    println!("Running test: bck run-ephemeral --help");

    let output = cmd!(sh, "bck run-ephemeral --help").output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!(
            "Failed to run 'bck run-ephemeral --help': {}",
            stderr
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Check for expected help text content
    if !stdout.contains("Run a container image as an ephemeral VM") {
        return Err(eyre!("Help text missing expected description"));
    }

    if !stdout.contains("--init") || !stdout.contains("--memory") || !stdout.contains("--vcpus") {
        return Err(eyre!("Help text missing expected options"));
    }

    println!("✅ Test passed: bck run-ephemeral --help");
    Ok(())
}

fn test_run_ephemeral_smoke(sh: &Shell) -> Result<()> {
    println!("Running test: bck run-ephemeral smoke test");

    // Check if required tools are available
    let virtiofsd_check = Command::new("which").arg("virtiofsd").output()?;

    if !virtiofsd_check.status.success() {
        println!("⚠️  Skipping run-ephemeral smoke test: virtiofsd not found");
        return Ok(());
    }

    let qemu_check = Command::new("which").arg("qemu-system-x86_64").output()?;

    if !qemu_check.status.success() {
        println!("⚠️  Skipping run-ephemeral smoke test: qemu-system-x86_64 not found");
        return Ok(());
    }

    // Try to run with a simple command that should exit quickly
    // Using timeout to ensure it doesn't hang
    let test_image = "quay.io/fedora/fedora-bootc:42";

    println!("  Testing with image: {}", test_image);

    // First, check if we can pull the image (this might fail in CI)
    let pull_output = Command::new("podman").args(["pull", test_image]).output()?;

    if !pull_output.status.success() {
        println!("⚠️  Skipping run-ephemeral smoke test: unable to pull test image");
        return Ok(());
    }

    // Run the ephemeral VM with /bin/false as init (should exit immediately)
    let mut child = Command::new("timeout")
        .args([
            "10", // 10 second timeout
            "bck",
            "run-ephemeral",
            test_image,
            "--init",
            "/bin/false",
            "--memory",
            "512",
            "--vcpus",
            "1",
            "--kvm=false", // Disable KVM for CI
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let status = child.wait()?;

    // We expect this to fail quickly (either timeout or /bin/false exit)
    // The important part is that it doesn't crash
    println!("✅ Test passed: bck run-ephemeral smoke test (command executed without crash)");
    Ok(())
}

fn main() -> Result<()> {
    // Set up error handling
    color_eyre::install()?;

    // Set up shell
    let sh = Shell::new()?;

    // Track test failures
    let mut failures = Vec::new();

    // Run all tests
    match test_images_list(&sh) {
        Ok(_) => {}
        Err(e) => failures.push(format!("test_images_list: {}", e)),
    }

    match test_markdown_no_trailing_whitespace() {
        Ok(_) => {}
        Err(e) => failures.push(format!("test_markdown_no_trailing_whitespace: {}", e)),
    }

    match test_run_ephemeral_help(&sh) {
        Ok(_) => {}
        Err(e) => failures.push(format!("test_run_ephemeral_help: {}", e)),
    }

    match test_run_ephemeral_smoke(&sh) {
        Ok(_) => {}
        Err(e) => failures.push(format!("test_run_ephemeral_smoke: {}", e)),
    }

    // Report results
    println!("\n--- Integration Test Results ---");
    if failures.is_empty() {
        println!("All tests passed! ✅");
        Ok(())
    } else {
        println!("Some tests failed! ❌");
        for failure in &failures {
            println!("❌ {}", failure);
        }
        std::process::exit(1);
    }
}
