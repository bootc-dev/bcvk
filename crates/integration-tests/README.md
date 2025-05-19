# Integration Tests for bootc-kit

This crate contains integration tests for the bootc-kit project.

## Running Tests

To run the integration tests, use:

```
just test-integration
```

## Adding New Tests

To add a new test:

1. Create a new function in `main.rs` that runs the test
2. Add the test to the main function's test list
3. The function should return a Result, with Ok() for success and Err() for failure

Example:

```rust
fn test_new_feature(sh: &Shell) -> Result<()> {
    println!("Running test: new feature");

    // Run command
    let output = cmd!(sh, "bck new-feature").output()?;

    // Check result
    if !output.status.success() {
        return Err(eyre!("Failed to run 'bck new-feature'"));
    }

    println!("✅ Test passed: new feature");
    Ok(())
}
```

Then add it to main():

```rust
match test_new_feature(&sh) {
    Ok(_) => {},
    Err(e) => failures.push(format!("test_new_feature: {}", e)),
}
```