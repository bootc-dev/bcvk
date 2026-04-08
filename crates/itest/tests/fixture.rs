//! Tiny test fixture binary for testing itest's harness.
//!
//! Registers a few tests with known output patterns so the integration
//! tests can verify capture, --emit-tmt, --list, etc.

#![allow(unsafe_code)]

#[itest::test_attr]
fn passing_test() -> itest::TestResult {
    println!("FIXTURE_STDOUT_PASS");
    eprintln!("FIXTURE_STDERR_PASS");
    Ok(())
}

#[itest::test_attr]
fn failing_test() -> itest::TestResult {
    println!("FIXTURE_STDOUT_FAIL");
    eprintln!("FIXTURE_STDERR_FAIL");
    Err("deliberate failure".into())
}

/// A test with rich metadata to verify it flows through to emitted formats.
#[itest::test_attr(
    timeout = "1h",
    needs_root,
    tags = ["slow", "network"],
    summary = "A test with rich metadata",
    needs_internet,
    flaky,
)]
fn meta_test() -> itest::TestResult {
    Ok(())
}

#[itest::test_attr]
async fn async_test() -> itest::TestResult {
    println!("FIXTURE_ASYNC");
    // Prove we're actually in a tokio runtime
    tokio::task::yield_now().await;
    Ok(())
}

fn parameterized_test(param: &str) -> itest::TestResult {
    println!("FIXTURE_PARAM={param}");
    Ok(())
}
itest::parameterized_integration_test!(parameterized_test);

fn main() {
    let config = itest::TestConfig {
        report_name: "itest-fixture".into(),
        suite_name: "fixture".into(),
        parameters: vec!["alpha".into(), "beta".into()],
    };

    itest::run_tests_with_config(config);
}
