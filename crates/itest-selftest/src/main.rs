//! Self-tests for the itest integration test framework.
//!
//! This binary exercises every major feature of itest by actually
//! running privileged tests inside bcvk VMs.  It is NOT a unit test
//! target — it requires a container image with this binary baked in.
//!
//! Build the image and run via the Justfile:
//!
//!   cd crates/itest-selftest && just

#![allow(unsafe_code)]

use anyhow::{ensure, Result};

mod privileged;

// ── Unprivileged tests ──────────────────────────────────────────────

/// Simplest possible test: proves registration and harness work.
fn selftest_register_and_pass() -> Result<()> {
    Ok(())
}
itest::integration_test!(selftest_register_and_pass);

/// Verify the test process can introspect its own environment.
fn selftest_env_sanity() -> Result<()> {
    ensure!(
        std::env::current_exe().is_ok(),
        "current_exe() should succeed"
    );
    Ok(())
}
itest::integration_test!(selftest_env_sanity);

// ── Parameterized tests ─────────────────────────────────────────────

/// Verifies that the parameter is actually forwarded and non-empty.
fn selftest_parameterized(param: &str) -> Result<()> {
    ensure!(!param.is_empty(), "parameter must not be empty");
    Ok(())
}
itest::parameterized_integration_test!(selftest_parameterized);

// ── Harness entry point ─────────────────────────────────────────────

fn main() {
    let config = itest::TestConfig {
        report_name: "itest-selftest".into(),
        suite_name: "selftest".into(),
        parameters: vec!["alpha".into(), "beta".into()],
    };

    itest::run_tests_with_config(config);
}
