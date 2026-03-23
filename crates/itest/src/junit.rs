//! Optional JUnit XML output.
//!
//! When the `JUNIT_OUTPUT` environment variable is set, test outcomes
//! are serialised to JUnit XML after all tests complete.  This is
//! useful for CI systems (GitHub Actions, tmt, etc.) that can ingest
//! JUnit results for display.

use quick_junit::{NonSuccessKind, Report, TestCase, TestCaseStatus, TestSuite};

/// Outcome of a single test, captured during execution.
pub(crate) struct TestOutcome {
    /// Test name.
    pub(crate) name: String,
    /// Wall-clock duration.
    pub(crate) duration: std::time::Duration,
    /// `Ok(())` on success, `Err(message)` on failure.
    pub(crate) result: Result<(), String>,
}

/// Write JUnit XML to `path`.
///
/// `report_name` is the top-level report identifier (e.g. the binary
/// name).  `suite_name` groups the test cases (e.g. `"integration"`).
pub(crate) fn write_junit(
    path: &str,
    report_name: &str,
    suite_name: &str,
    outcomes: &[TestOutcome],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut report = Report::new(report_name);
    let mut suite = TestSuite::new(suite_name);

    for outcome in outcomes {
        let status = match &outcome.result {
            Ok(()) => TestCaseStatus::success(),
            Err(msg) => {
                let mut status = TestCaseStatus::non_success(NonSuccessKind::Failure);
                status.set_message(msg.clone());
                status
            }
        };
        let mut tc = TestCase::new(outcome.name.clone(), status);
        tc.set_time(outcome.duration);
        suite.add_test_case(tc);
    }

    report.add_test_suite(suite);
    let xml = report.to_string()?;
    std::fs::write(path, xml)?;
    eprintln!("JUnit XML written to {path}");
    Ok(())
}
