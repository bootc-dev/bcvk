// Integration test suite for bootc-kit
// This binary runs various integration tests for the bootc-kit project

use color_eyre::eyre::{eyre, Result};
use serde_json::Value;
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
