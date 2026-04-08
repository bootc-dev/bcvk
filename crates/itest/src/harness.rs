//! Test harness that wires libtest-mimic, distributed slices, and
//! optional JUnit output together.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use libtest_mimic::{Arguments, Trial};

use crate::junit::{write_junit, TestOutcome};
use crate::{image_to_test_suffix, INTEGRATION_TESTS, PARAMETERIZED_INTEGRATION_TESTS};

/// Default tmt test timeout, matching the nextest integration profile.
const TMT_DEFAULT_DURATION: &str = "20m";

/// Environment variable set in the child process during fork-exec
/// capture to prevent infinite recursion.
const SUBPROCESS_ENV: &str = "ITEST_SUBPROCESS";

/// Per-project configuration for the test harness.
#[derive(Debug, Clone)]
pub struct TestConfig {
    /// Name used in JUnit XML reports (e.g. the binary name).
    /// Defaults to `"integration-tests"`.
    pub report_name: String,

    /// Suite name inside JUnit XML.  Defaults to `"integration"`.
    pub suite_name: String,

    /// Parameter values for [`ParameterizedIntegrationTest`]s.
    ///
    /// Each parameterised test is expanded once per entry.  For
    /// image-based testing this is typically a list of container
    /// image references.  If empty, parameterised tests are skipped.
    pub parameters: Vec<String>,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            report_name: "integration-tests".into(),
            suite_name: "integration".into(),
            parameters: Vec::new(),
        }
    }
}

/// Run all registered tests using default configuration and no
/// parameters.
///
/// Equivalent to `run_tests_with_config(TestConfig::default())`.
pub fn run_tests() -> ! {
    run_tests_with_config(TestConfig::default())
}

/// Determine whether we should capture output via fork-exec-self.
///
/// Returns `true` when:
/// - Not already inside a subprocess (`ITEST_SUBPROCESS` not set)
/// - Not running under a per-test runner (nextest, tmt)
/// - Not explicitly suppressed (`ITEST_NOCAPTURE=1`)
fn should_capture() -> bool {
    std::env::var_os(SUBPROCESS_ENV).is_none()
        && std::env::var_os("NEXTEST").is_none()
        && std::env::var_os("TMT_TEST_DATA").is_none()
        && std::env::var_os("ITEST_NOCAPTURE").is_none()
}

/// Run all registered tests with the given configuration.
///
/// This function collects tests from the global distributed slices,
/// expands parameterised variants, runs them through libtest-mimic,
/// optionally writes JUnit XML, and exits the process.
///
/// # Output capture
///
/// When not running under an external test runner (nextest, tmt),
/// the harness automatically captures output by re-executing itself
/// as a subprocess for each test.  This prevents interleaved output
/// from parallel tests.  Captured output is only shown for failing
/// tests, matching `cargo test` default behaviour.
///
/// To disable capture (e.g. for debugging), set `ITEST_NOCAPTURE=1`.
///
/// # Test metadata generation
///
/// Instead of running tests, the binary can emit metadata for
/// external test runners:
///
/// - `--emit-tmt <dir>` — FMF metadata for [tmt](https://tmt.readthedocs.io)
/// - `--emit-autopkgtest <dir>` — DEP-8 `control` file for
///   [autopkgtest](https://wiki.debian.org/ContinuousIntegration/autopkgtest)
///
/// Each registered test becomes an entry that invokes
/// `<binary> --exact <test_name>`.
pub fn run_tests_with_config(config: TestConfig) -> ! {
    let raw_args: Vec<String> = std::env::args().collect();

    // --vm-jobserver -- <command...>
    // Create a VM memory jobserver, then exec the given command
    // with the pipe fds inherited.  Used to wrap nextest:
    //   my-tests --vm-jobserver -- cargo nextest run ...
    if let Some(pos) = raw_args.iter().position(|a| a == "--vm-jobserver") {
        let rest: Vec<&str> = raw_args[pos + 1..]
            .iter()
            .skip_while(|a| a.as_str() == "--")
            .map(|s| s.as_str())
            .collect();
        if rest.is_empty() {
            eprintln!("error: --vm-jobserver requires a command after --");
            std::process::exit(1);
        }
        exec_with_jobserver(&rest);
    }

    // Check for metadata emission flags before libtest-mimic parses args.
    for (flag, emitter) in [
        ("--emit-tmt", emit_tmt as fn(&TestConfig, &str, &str) -> _),
        ("--emit-autopkgtest", emit_autopkgtest as _),
    ] {
        if let Some(pos) = raw_args.iter().position(|a| a == flag) {
            let dir = raw_args.get(pos + 1).cloned().unwrap_or_else(|| {
                eprintln!("error: {flag} requires a directory argument");
                std::process::exit(1);
            });
            let binary = &raw_args[0];
            if let Err(e) = emitter(&config, binary, &dir) {
                eprintln!("error: {flag} failed: {e}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }
    }

    let capture = should_capture();

    let args = Arguments::from_args();
    let outcomes: Arc<Mutex<Vec<TestOutcome>>> = Arc::new(Mutex::new(Vec::new()));

    let mut tests: Vec<Trial> = Vec::new();

    // The binary path for fork-exec; only resolved when capturing.
    let self_exe: Option<Arc<std::path::PathBuf>> = if capture {
        match std::env::current_exe() {
            Ok(p) => Some(Arc::new(p)),
            Err(e) => {
                eprintln!("warning: cannot resolve current_exe, disabling output capture: {e}");
                None
            }
        }
    } else {
        None
    };

    // Collect plain tests
    for t in INTEGRATION_TESTS.iter() {
        let name = t.name.to_owned();
        let outcomes = Arc::clone(&outcomes);

        let trial = if let Some(ref exe) = self_exe {
            // Capture mode: fork-exec self with --exact
            let exe = Arc::clone(exe);
            let test_name = t.name.to_owned();
            Trial::test(t.name, move || {
                run_captured(&exe, &test_name, &name, &outcomes)
            })
        } else {
            // Direct mode: run in-process
            let f = t.f;
            Trial::test(t.name, move || run_direct(f, &name, &outcomes))
        };
        tests.push(trial);
    }

    // Expand parameterised tests
    for pt in PARAMETERIZED_INTEGRATION_TESTS.iter() {
        for param in &config.parameters {
            let param = param.clone();
            let suffix = image_to_test_suffix(&param);
            let test_name = format!("{}_{}", pt.name, suffix);
            let display_name = test_name.clone();
            let outcomes = Arc::clone(&outcomes);

            let trial = if let Some(ref exe) = self_exe {
                let exe = Arc::clone(exe);
                let tn = test_name.clone();
                Trial::test(test_name, move || {
                    run_captured(&exe, &tn, &display_name, &outcomes)
                })
            } else {
                let f = pt.f;
                Trial::test(test_name, move || {
                    run_direct_param(f, &param, &display_name, &outcomes)
                })
            };
            tests.push(trial);
        }
    }

    let conclusion = libtest_mimic::run(&args, tests);

    // Write JUnit XML if requested
    if let Ok(path) = std::env::var("JUNIT_OUTPUT") {
        if let Err(e) = write_junit(
            &path,
            &config.report_name,
            &config.suite_name,
            &outcomes.lock().unwrap_or_else(|e| e.into_inner()),
        ) {
            eprintln!("warning: failed to write JUnit XML to {path}: {e}");
        }
    }

    std::process::exit(if conclusion.has_failed() { 101 } else { 0 });
}

/// Run a test function directly in-process (used when capture is not needed).
fn run_direct(
    f: crate::TestFn,
    name: &str,
    outcomes: &Mutex<Vec<TestOutcome>>,
) -> Result<(), libtest_mimic::Failed> {
    let start = Instant::now();
    let result = f();
    let duration = start.elapsed();
    let outcome = TestOutcome {
        name: name.to_owned(),
        duration,
        result: result.as_ref().map(|_| ()).map_err(|e| format!("{e:?}")),
    };
    outcomes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(outcome);
    result.map_err(|e| format!("{e:?}").into())
}

/// Run a parameterised test function directly in-process.
fn run_direct_param(
    f: crate::ParameterizedTestFn,
    param: &str,
    name: &str,
    outcomes: &Mutex<Vec<TestOutcome>>,
) -> Result<(), libtest_mimic::Failed> {
    let start = Instant::now();
    let result = f(param);
    let duration = start.elapsed();
    let outcome = TestOutcome {
        name: name.to_owned(),
        duration,
        result: result.as_ref().map(|_| ()).map_err(|e| format!("{e:?}")),
    };
    outcomes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(outcome);
    result.map_err(|e| format!("{e:?}").into())
}

/// Run a test by re-executing self as a subprocess, capturing output.
///
/// The child is invoked with `ITEST_SUBPROCESS=1` to prevent recursion,
/// plus `--exact <test_name> --nocapture` so libtest-mimic runs just
/// the one test and doesn't try to capture (the parent is doing it).
///
/// On success, captured output is discarded.  On failure, it is
/// included in the error message so libtest-mimic displays it.
fn run_captured(
    exe: &std::path::Path,
    test_name: &str,
    display_name: &str,
    outcomes: &Mutex<Vec<TestOutcome>>,
) -> Result<(), libtest_mimic::Failed> {
    let start = Instant::now();

    let output = std::process::Command::new(exe)
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .env(SUBPROCESS_ENV, "1")
        .env_remove("JUNIT_OUTPUT") // parent handles JUnit
        .output();

    let duration = start.elapsed();

    match output {
        Ok(output) => {
            let success = output.status.success();
            let outcome = TestOutcome {
                name: display_name.to_owned(),
                duration,
                result: if success {
                    Ok(())
                } else {
                    Err(format!("exit status: {}", output.status))
                },
            };
            outcomes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(outcome);

            if success {
                Ok(())
            } else {
                // Include captured output in the failure message
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let mut msg = format!("test failed ({})\n", output.status);
                if !stdout.is_empty() {
                    msg.push_str("--- stdout ---\n");
                    msg.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    msg.push_str("--- stderr ---\n");
                    msg.push_str(&stderr);
                }
                Err(msg.into())
            }
        }
        Err(e) => {
            let outcome = TestOutcome {
                name: display_name.to_owned(),
                duration,
                result: Err(format!("failed to spawn subprocess: {e}")),
            };
            outcomes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(outcome);
            Err(format!("failed to spawn subprocess: {e}").into())
        }
    }
}

/// Generate tmt FMF test metadata from registered tests.
///
/// Creates a `tests.fmf` file in `dir` with one entry per test.
/// Each test's `test:` field invokes the binary with `--exact`.
fn emit_tmt(
    config: &TestConfig,
    binary: &str,
    dir: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let dir = std::path::Path::new(dir);
    std::fs::create_dir_all(dir)?;

    let tests: Vec<_> = INTEGRATION_TESTS
        .iter()
        .map(|t| (t.name, &t.meta))
        .collect();
    let param_tests: Vec<_> = PARAMETERIZED_INTEGRATION_TESTS
        .iter()
        .map(|t| (t.name, &t.meta))
        .collect();

    let content = format_tmt_fmf(binary, &tests, &param_tests, &config.parameters);

    let output_path = dir.join("tests.fmf");
    std::fs::write(&output_path, &content)?;
    eprintln!("tmt metadata written to {}", output_path.display());

    Ok(())
}

/// Format tmt FMF content from test names and metadata.
///
/// Separated from [`emit_tmt`] for testability — no I/O or global state.
fn format_tmt_fmf(
    binary: &str,
    tests: &[(&str, &crate::TestMeta)],
    parameterized_tests: &[(&str, &crate::TestMeta)],
    parameters: &[String],
) -> String {
    use std::fmt::Write;

    let binary_name = binary_basename(binary);

    let mut content = String::new();
    let _ = writeln!(content, "# THIS IS GENERATED CODE - DO NOT EDIT");
    let _ = writeln!(content, "# Generated by: {binary_name} --emit-tmt");
    let _ = writeln!(content);

    for &(name, meta) in tests {
        write_tmt_entry(&mut content, binary_name, name, name, meta);
    }

    for &(name, meta) in parameterized_tests {
        for param in parameters {
            let suffix = image_to_test_suffix(param);
            let test_name = format!("{name}_{suffix}");
            let summary = format!("{name} [{param}]");
            write_tmt_entry(&mut content, binary_name, &test_name, &summary, meta);
        }
    }

    content
}

/// Write a single tmt FMF test entry, including metadata fields.
fn write_tmt_entry(
    w: &mut String,
    binary_name: &str,
    test_name: &str,
    summary: &str,
    meta: &crate::TestMeta,
) {
    use std::fmt::Write;

    let _ = writeln!(w, "/{test_name}:");
    let _ = writeln!(w, "  summary: {}", meta.summary.unwrap_or(summary));
    let _ = writeln!(w, "  test: {binary_name} --exact {test_name}");
    let duration = meta.timeout.unwrap_or(TMT_DEFAULT_DURATION);
    let _ = writeln!(w, "  duration: {duration}");
    if !meta.tags.is_empty() {
        let _ = writeln!(w, "  tag: [{}]", meta.tags.join(", "));
    }
    if meta.flaky {
        let _ = writeln!(w, "  result: xfail");
    }
    let _ = writeln!(w);
}

/// Generate autopkgtest (DEP-8) control file from registered tests.
///
/// Creates a `control` file in `dir` with one stanza per test.
/// Each stanza uses `Test-Command:` to invoke the binary with `--exact`.
fn emit_autopkgtest(
    config: &TestConfig,
    binary: &str,
    dir: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let dir = std::path::Path::new(dir);
    std::fs::create_dir_all(dir)?;

    let tests: Vec<_> = INTEGRATION_TESTS
        .iter()
        .map(|t| (t.name, &t.meta))
        .collect();
    let param_tests: Vec<_> = PARAMETERIZED_INTEGRATION_TESTS
        .iter()
        .map(|t| (t.name, &t.meta))
        .collect();

    let content = format_autopkgtest_control(binary, &tests, &param_tests, &config.parameters);

    let output_path = dir.join("control");
    std::fs::write(&output_path, &content)?;
    eprintln!("autopkgtest control written to {}", output_path.display());

    Ok(())
}

/// Format autopkgtest (DEP-8) control file content.
///
/// Each test becomes a stanza with `Test-Command:` and `Features: test-name`.
/// Stanzas are separated by blank lines per the DEP-8 spec.
///
/// Separated from [`emit_autopkgtest`] for testability.
fn format_autopkgtest_control(
    binary: &str,
    tests: &[(&str, &crate::TestMeta)],
    parameterized_tests: &[(&str, &crate::TestMeta)],
    parameters: &[String],
) -> String {
    use std::fmt::Write;

    let binary_name = binary_basename(binary);

    let mut content = String::new();
    let _ = writeln!(content, "# THIS IS GENERATED CODE - DO NOT EDIT");
    let _ = writeln!(content, "# Generated by: {binary_name} --emit-autopkgtest");

    for &(name, meta) in tests {
        write_autopkgtest_stanza(&mut content, binary_name, name, meta);
    }

    for &(name, meta) in parameterized_tests {
        for param in parameters {
            let suffix = image_to_test_suffix(param);
            let test_name = format!("{name}_{suffix}");
            write_autopkgtest_stanza(&mut content, binary_name, &test_name, meta);
        }
    }

    content
}

/// Write a single autopkgtest (DEP-8) stanza, including metadata fields.
fn write_autopkgtest_stanza(
    w: &mut String,
    binary_name: &str,
    test_name: &str,
    meta: &crate::TestMeta,
) {
    use std::fmt::Write;

    let _ = writeln!(w);
    let _ = writeln!(w, "Test-Command: {binary_name} --exact {test_name}");
    let _ = writeln!(w, "Features: test-name={test_name}");

    // Build restrictions list from metadata
    let mut restrictions = Vec::new();
    if meta.needs_root {
        restrictions.push("needs-root");
    }
    match meta.isolation {
        crate::Isolation::None => {}
        crate::Isolation::Container => restrictions.push("isolation-container"),
        crate::Isolation::Machine => restrictions.push("isolation-machine"),
    }
    if meta.needs_internet {
        restrictions.push("needs-internet");
    }
    if meta.flaky {
        restrictions.push("flaky");
    }
    if !restrictions.is_empty() {
        let _ = writeln!(w, "Restrictions: {}", restrictions.join(", "));
    }
    if !meta.tags.is_empty() {
        let _ = writeln!(w, "Classes: {}", meta.tags.join(", "));
    }
}

/// Extract just the filename from a binary path.
fn binary_basename(binary: &str) -> &str {
    std::path::Path::new(binary)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(binary)
}

/// Create a VM jobserver and exec the given command with the pipe
/// fds inherited.  Does not return on success.
fn exec_with_jobserver(cmd: &[&str]) -> ! {
    use crate::resources::{compute_token_count, VmJobserver};

    let tokens = compute_token_count();
    let js = VmJobserver::create(tokens).unwrap_or_else(|e| {
        eprintln!("error: failed to create VM jobserver: {e}");
        std::process::exit(1);
    });
    let (r, w) = js.fds();

    let budget_mib = tokens * crate::resources::TOKEN_MIB;
    eprintln!(
        "itest: VM jobserver: {tokens} token(s) ({budget_mib} MiB budget), \
         fds ({r},{w})"
    );

    // Keep the pipe fds open for the child process we're about to exec into.
    // VmJobserver stores raw fds — no Drop impl — but ManuallyDrop makes
    // the intent explicit and avoids clippy::forget_non_drop.
    let _js = std::mem::ManuallyDrop::new(js);

    // exec the command with ITEST_VM_FDS set
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(cmd[0])
        .args(&cmd[1..])
        .env("ITEST_VM_FDS", format!("{r},{w}"))
        .exec();

    eprintln!("error: exec {:?} failed: {err}", cmd[0]);
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Isolation, TestMeta};

    const DEFAULT: TestMeta = TestMeta::EMPTY;

    const ROOT_META: TestMeta = TestMeta {
        needs_root: true,
        ..TestMeta::EMPTY
    };

    const RICH_META: TestMeta = TestMeta {
        timeout: Some("1h"),
        needs_root: true,
        isolation: Isolation::Machine,
        tags: &["slow", "network"],
        summary: Some("A slow network test"),
        needs_internet: true,
        flaky: true,
    };

    // ── tmt FMF ─────────────────────────────────────────────────────

    #[test]
    fn tmt_fmf_plain_tests() {
        let content = format_tmt_fmf(
            "/path/to/my-binary",
            &[("test_foo", &DEFAULT), ("test_bar", &DEFAULT)],
            &[],
            &[],
        );

        assert!(content.starts_with("# THIS IS GENERATED CODE"));
        assert!(content.contains("# Generated by: my-binary --emit-tmt"));
        assert!(content.contains("/test_foo:"));
        assert!(content.contains("  summary: test_foo"));
        assert!(content.contains("  test: my-binary --exact test_foo"));
        assert!(content.contains("  duration: 20m"));
        assert!(content.contains("/test_bar:"));
    }

    #[test]
    fn tmt_fmf_parameterized_tests() {
        let content = format_tmt_fmf(
            "my-binary",
            &[],
            &[("test_multi", &DEFAULT)],
            &["quay.io/img:v1".into(), "localhost/other".into()],
        );

        assert!(content.contains("/test_multi_quay_io_img_v1:"));
        assert!(content.contains("  summary: test_multi [quay.io/img:v1]"));
        assert!(content.contains("  test: my-binary --exact test_multi_quay_io_img_v1"));
        assert!(content.contains("/test_multi_localhost_other:"));
    }

    #[test]
    fn tmt_fmf_with_metadata() {
        let content = format_tmt_fmf("bin", &[("test_rich", &RICH_META)], &[], &[]);

        assert!(content.contains("  duration: 1h"));
        assert!(content.contains("  summary: A slow network test"));
        assert!(content.contains("  tag: [slow, network]"));
        assert!(content.contains("  result: xfail"));
    }

    #[test]
    fn tmt_fmf_no_extra_fields_with_defaults() {
        let content = format_tmt_fmf("bin", &[("test_plain", &DEFAULT)], &[], &[]);

        // Default metadata should not emit tag or result
        assert!(!content.contains("  tag:"));
        assert!(!content.contains("  result:"));
    }

    #[test]
    fn tmt_fmf_empty() {
        let content = format_tmt_fmf("bin", &[], &[], &[]);
        assert!(content.contains("# THIS IS GENERATED CODE"));
        assert!(!content.contains("  test:"));
    }

    #[test]
    fn tmt_fmf_strips_path() {
        let content = format_tmt_fmf("/usr/local/bin/my-tests", &[("a_test", &DEFAULT)], &[], &[]);
        assert!(content.contains("  test: my-tests --exact a_test"));
        assert!(!content.contains("/usr/local/bin"));
    }

    // ── autopkgtest ─────────────────────────────────────────────────

    #[test]
    fn autopkgtest_plain_tests() {
        let content = format_autopkgtest_control(
            "/usr/bin/my-binary",
            &[("test_foo", &ROOT_META), ("test_bar", &ROOT_META)],
            &[],
            &[],
        );

        assert!(content.contains("# THIS IS GENERATED CODE"));
        assert!(content.contains("Test-Command: my-binary --exact test_foo"));
        assert!(content.contains("Features: test-name=test_foo"));
        assert!(content.contains("Restrictions: needs-root"));
        assert!(content.contains("Test-Command: my-binary --exact test_bar"));
    }

    #[test]
    fn autopkgtest_with_metadata() {
        let content = format_autopkgtest_control("bin", &[("test_rich", &RICH_META)], &[], &[]);

        assert!(
            content.contains("Restrictions: needs-root, isolation-machine, needs-internet, flaky")
        );
        assert!(content.contains("Classes: slow, network"));
    }

    #[test]
    fn autopkgtest_no_restrictions_when_empty() {
        let content = format_autopkgtest_control("bin", &[("test_plain", &DEFAULT)], &[], &[]);

        // With default meta (no needs_root, no isolation, etc.),
        // there should be no Restrictions line
        assert!(!content.contains("Restrictions:"));
        assert!(!content.contains("Classes:"));
    }

    #[test]
    fn autopkgtest_parameterized() {
        let content = format_autopkgtest_control(
            "my-binary",
            &[],
            &[("test_multi", &ROOT_META)],
            &["img:v1".into()],
        );

        assert!(content.contains("Test-Command: my-binary --exact test_multi_img_v1"));
        assert!(content.contains("Features: test-name=test_multi_img_v1"));
    }

    #[test]
    fn autopkgtest_stanzas_separated_by_blank_lines() {
        let content =
            format_autopkgtest_control("bin", &[("a", &ROOT_META), ("b", &ROOT_META)], &[], &[]);

        assert!(
            content.contains("Restrictions: needs-root\n\nTest-Command:"),
            "stanzas must be separated by blank lines, got:\n{content}"
        );
    }

    #[test]
    fn autopkgtest_empty() {
        let content = format_autopkgtest_control("bin", &[], &[], &[]);
        assert!(content.contains("# THIS IS GENERATED CODE"));
        assert!(!content.contains("Test-Command:"));
    }

    #[test]
    fn autopkgtest_isolation_container() {
        let meta = TestMeta {
            isolation: Isolation::Container,
            ..TestMeta::EMPTY
        };
        let content = format_autopkgtest_control("bin", &[("test_c", &meta)], &[], &[]);
        assert!(content.contains("Restrictions: isolation-container"));
    }

    // ── misc ────────────────────────────────────────────────────────

    #[test]
    fn capture_disabled_under_nextest() {
        assert_eq!(SUBPROCESS_ENV, "ITEST_SUBPROCESS");
    }

    #[test]
    fn tmt_default_duration_is_valid() {
        assert!(TMT_DEFAULT_DURATION.ends_with('m') || TMT_DEFAULT_DURATION.ends_with('h'));
        let numeric: String = TMT_DEFAULT_DURATION
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        assert!(!numeric.is_empty(), "duration should start with digits");
    }
}
