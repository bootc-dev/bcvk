//! Test harness that wires libtest-mimic, distributed slices, and
//! optional JUnit output together.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use libtest_mimic::{Arguments, Trial};

use crate::junit::{write_junit, TestOutcome};
use crate::{image_to_test_suffix, INTEGRATION_TESTS, PARAMETERIZED_INTEGRATION_TESTS};

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

/// Run all registered tests with the given configuration.
///
/// This function collects tests from the global distributed slices,
/// expands parameterised variants, runs them through libtest-mimic,
/// optionally writes JUnit XML, and exits the process.
pub fn run_tests_with_config(config: TestConfig) -> ! {
    let args = Arguments::from_args();
    let outcomes: Arc<Mutex<Vec<TestOutcome>>> = Arc::new(Mutex::new(Vec::new()));

    let mut tests: Vec<Trial> = Vec::new();

    // Collect plain tests
    for t in INTEGRATION_TESTS.iter() {
        let f = t.f;
        let name = t.name.to_owned();
        let outcomes = Arc::clone(&outcomes);
        tests.push(Trial::test(t.name, move || {
            let start = Instant::now();
            let result = f();
            let duration = start.elapsed();
            let outcome = TestOutcome {
                name,
                duration,
                result: result.as_ref().map(|_| ()).map_err(|e| format!("{e:?}")),
            };
            outcomes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(outcome);
            result.map_err(|e| format!("{e:?}").into())
        }));
    }

    // Expand parameterised tests
    for pt in PARAMETERIZED_INTEGRATION_TESTS.iter() {
        for param in &config.parameters {
            let param = param.clone();
            let suffix = image_to_test_suffix(&param);
            let test_name = format!("{}_{}", pt.name, suffix);
            let display_name = test_name.clone();
            let f = pt.f;
            let outcomes = Arc::clone(&outcomes);
            tests.push(Trial::test(test_name, move || {
                let start = Instant::now();
                let result = f(&param);
                let duration = start.elapsed();
                let outcome = TestOutcome {
                    name: display_name,
                    duration,
                    result: result.as_ref().map(|_| ()).map_err(|e| format!("{e:?}")),
                };
                outcomes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(outcome);
                result.map_err(|e| format!("{e:?}").into())
            }));
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
