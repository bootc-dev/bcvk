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

mod privileged;

// ── Unprivileged tests ──────────────────────────────────────────────

/// Simplest possible test: proves registration and harness work.
fn selftest_register_and_pass() -> itest::TestResult {
    Ok(())
}
itest::integration_test!(selftest_register_and_pass);

/// Verify the test process can introspect its own environment.
fn selftest_env_sanity() -> itest::TestResult {
    let _ = std::env::current_exe()?;
    Ok(())
}
itest::integration_test!(selftest_env_sanity);

// ── Parameterized tests ─────────────────────────────────────────────

/// Verifies that the parameter is actually forwarded and non-empty.
fn selftest_parameterized(param: &str) -> itest::TestResult {
    if param.is_empty() {
        return Err("parameter must not be empty".into());
    }
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
