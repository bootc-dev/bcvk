//! Integration tests for itest's harness features.
//!
//! These tests run the `itest-fixture` binary (a tiny test harness
//! with known tests) and verify capture, --emit-tmt, and --list
//! behaviour.

use std::process::Command;

/// Path to the fixture binary, set by cargo.
fn fixture_bin() -> String {
    // cargo sets CARGO_BIN_EXE_<name> for [[bin]] targets in the same crate
    env!("CARGO_BIN_EXE_itest-fixture").to_string()
}

/// Run the fixture with the given args and env.
fn run_fixture(args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = Command::new(fixture_bin());
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    // Ensure we don't inherit these from the outer test runner
    cmd.env_remove("NEXTEST");
    cmd.env_remove("TMT_TEST_DATA");
    cmd.output()
        .unwrap_or_else(|e| panic!("failed to run fixture: {e}"))
}

// ── --list ──────────────────────────────────────────────────────────

#[test]
fn list_shows_all_tests() {
    let out = run_fixture(&["--list"], &[("ITEST_SUBPROCESS", "1")]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(out.status.success(), "fixture --list failed: {stdout}");
    assert!(stdout.contains("passing_test: test"));
    assert!(stdout.contains("failing_test: test"));
    assert!(stdout.contains("async_test: test"));
    assert!(stdout.contains("parameterized_test_alpha: test"));
    assert!(stdout.contains("parameterized_test_beta: test"));
}

// ── async tests ─────────────────────────────────────────────────────

#[test]
fn async_test_runs_with_tokio_runtime() {
    // The async_test fixture uses tokio::task::yield_now().await,
    // proving it has a real tokio runtime.
    let out = run_fixture(&["--exact", "async_test"], &[("ITEST_SUBPROCESS", "1")]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(out.status.success(), "async test should pass");
    assert!(
        stdout.contains("FIXTURE_ASYNC"),
        "async test should produce output, got:\n{stdout}"
    );
}

// ── fork-exec capture ───────────────────────────────────────────────

#[test]
fn capture_hides_passing_test_output() {
    // Run only the passing test, with capture active (no NEXTEST, no
    // ITEST_SUBPROCESS).
    let out = run_fixture(&["--exact", "passing_test"], &[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(out.status.success(), "passing test should succeed");
    // The fixture prints FIXTURE_STDOUT_PASS, but capture should hide it
    assert!(
        !stdout.contains("FIXTURE_STDOUT_PASS"),
        "captured output should not appear in stdout for passing test, got:\n{stdout}"
    );
    assert!(
        !stderr.contains("FIXTURE_STDERR_PASS"),
        "captured output should not appear in stderr for passing test, got:\n{stderr}"
    );
}

#[test]
fn capture_shows_failing_test_output() {
    // Run only the failing test with capture active
    let out = run_fixture(&["--exact", "failing_test"], &[]);

    assert!(!out.status.success(), "failing test should fail");

    // The failure output should include the captured stdout/stderr
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("FIXTURE_STDOUT_FAIL"),
        "failure output should contain captured stdout, got:\n{combined}"
    );
    assert!(
        combined.contains("FIXTURE_STDERR_FAIL"),
        "failure output should contain captured stderr, got:\n{combined}"
    );
}

#[test]
fn nocapture_passes_output_through() {
    // With ITEST_NOCAPTURE=1, output should pass through directly
    let out = run_fixture(&["--exact", "passing_test"], &[("ITEST_NOCAPTURE", "1")]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(out.status.success());
    assert!(
        stdout.contains("FIXTURE_STDOUT_PASS"),
        "with ITEST_NOCAPTURE, output should pass through, got:\n{stdout}"
    );
}

#[test]
fn subprocess_env_runs_directly() {
    // With ITEST_SUBPROCESS=1, the test runs directly (no fork-exec).
    // Output should pass through since there's no capture layer.
    let out = run_fixture(
        &["--exact", "passing_test", "--nocapture"],
        &[("ITEST_SUBPROCESS", "1")],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(out.status.success());
    assert!(
        stdout.contains("FIXTURE_STDOUT_PASS"),
        "in subprocess mode, output should pass through, got:\n{stdout}"
    );
}

// ── --emit-tmt ──────────────────────────────────────────────────────

#[test]
fn emit_tmt_generates_valid_fmf() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_str().unwrap();

    let out = run_fixture(&["--emit-tmt", dir_path], &[("ITEST_SUBPROCESS", "1")]);
    assert!(
        out.status.success(),
        "--emit-tmt failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let fmf = std::fs::read_to_string(dir.path().join("tests.fmf")).unwrap();

    // Header
    assert!(fmf.contains("# THIS IS GENERATED CODE"));

    // Plain tests
    assert!(fmf.contains("/passing_test:"));
    assert!(fmf.contains("  test: itest-fixture --exact passing_test"));
    assert!(fmf.contains("/failing_test:"));
    assert!(fmf.contains("  test: itest-fixture --exact failing_test"));

    // Parameterised tests
    assert!(fmf.contains("/parameterized_test_alpha:"));
    assert!(fmf.contains("  summary: parameterized_test [alpha]"));
    assert!(fmf.contains("/parameterized_test_beta:"));

    // All entries have duration
    assert!(fmf.contains("  duration: 20m"));
}

#[test]
fn emit_tmt_creates_directory() {
    let parent = tempfile::tempdir().unwrap();
    let nested = parent.path().join("deep").join("nested");
    let dir_path = nested.to_str().unwrap();

    let out = run_fixture(&["--emit-tmt", dir_path], &[("ITEST_SUBPROCESS", "1")]);
    assert!(out.status.success());
    assert!(nested.join("tests.fmf").exists());
}

// ── --emit-autopkgtest ──────────────────────────────────────────────

#[test]
fn emit_autopkgtest_generates_control() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_str().unwrap();

    let out = run_fixture(
        &["--emit-autopkgtest", dir_path],
        &[("ITEST_SUBPROCESS", "1")],
    );
    assert!(
        out.status.success(),
        "--emit-autopkgtest failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let control = std::fs::read_to_string(dir.path().join("control")).unwrap();

    // Header
    assert!(control.contains("# THIS IS GENERATED CODE"));

    // Plain tests — each has a Test-Command stanza
    assert!(control.contains("Test-Command: itest-fixture --exact passing_test"));
    assert!(control.contains("Test-Command: itest-fixture --exact failing_test"));

    // Parameterised tests
    assert!(control.contains("Test-Command: itest-fixture --exact parameterized_test_alpha"));
    assert!(control.contains("Test-Command: itest-fixture --exact parameterized_test_beta"));

    // Features field present
    assert!(control.contains("Features: test-name=passing_test"));

    // meta_test has rich metadata that should map to DEP-8 fields
    assert!(control.contains("Test-Command: itest-fixture --exact meta_test"));
    assert!(
        control.contains("Restrictions: needs-root, needs-internet, flaky"),
        "meta_test should have rich restrictions, got:\n{control}"
    );
    assert!(
        control.contains("Classes: slow, network"),
        "meta_test should have Classes from tags, got:\n{control}"
    );
}

#[test]
fn emit_tmt_includes_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_str().unwrap();

    let out = run_fixture(&["--emit-tmt", dir_path], &[("ITEST_SUBPROCESS", "1")]);
    assert!(out.status.success());

    let fmf = std::fs::read_to_string(dir.path().join("tests.fmf")).unwrap();

    // meta_test should have custom duration, tags, summary, and result
    assert!(
        fmf.contains("  duration: 1h"),
        "meta_test should have 1h duration, got:\n{fmf}"
    );
    assert!(
        fmf.contains("  summary: A test with rich metadata"),
        "meta_test should have custom summary, got:\n{fmf}"
    );
    assert!(
        fmf.contains("  tag: [slow, network]"),
        "meta_test should have tags, got:\n{fmf}"
    );
    assert!(
        fmf.contains("  result: xfail"),
        "meta_test should have xfail result, got:\n{fmf}"
    );
}
