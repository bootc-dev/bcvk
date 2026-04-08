//! Reusable integration test infrastructure for bootc-dev projects.
//!
//! This crate provides a common test harness built on [`libtest_mimic`] with
//! automatic test registration via [`linkme`] distributed slices.  It is
//! designed to be shared across repositories such as bcvk, ostree, bootc, and
//! composefs-rs, reducing duplication of test infrastructure code.
//!
//! # Core concepts
//!
//! ## Test registration
//!
//! Tests are registered at link time using the [`integration_test!`] and
//! [`parameterized_integration_test!`] macros.  No manual lists in `main()`.
//!
//! ## Privilege tiers
//!
//! Tests that need root can use [`privileged_test!`] or [`booted_test!`].
//! When run without root these macros automatically re-dispatch the test
//! inside a bcvk VM, so the same binary works both on a developer laptop
//! and inside a tmt / autopkgtest / CI environment.
//!
//! ## Error types
//!
//! Test functions return [`TestResult`], which uses
//! `Box<dyn Error + Send + Sync>` as the error type.  This is compatible
//! with all major error libraries — `anyhow`, `color_eyre`, `eyre`, and
//! plain `std::io::Error` all convert via `?` without any wrapper.
//!
//! ## Harness
//!
//! Call [`run_tests`] (or [`run_tests_with_config`]) from your `main()` to
//! collect tests, expand parameterised variants, run them via libtest-mimic,
//! and optionally write JUnit XML.

// linkme requires unsafe for distributed slices
#![allow(unsafe_code)]

mod harness;
mod junit;
mod privilege;
mod resources;

pub use harness::{run_tests, run_tests_with_config, TestConfig};
pub use privilege::{require_root, DispatchMode};

// Re-export dependencies used by our macros so consumers don't need
// to add them to their own Cargo.toml.
#[doc(hidden)]
pub use linkme;
#[doc(hidden)]
pub use paste;
#[doc(hidden)]
pub use tokio;

// Re-export the proc-macro attribute.  Named `test_attr` to avoid
// conflicts with both the `#[test]` prelude attribute and the
// `integration_test!` declarative macro.
pub use itest_macros::integration_test as test_attr;

/// Error type for integration tests.
///
/// Compatible with all major error libraries:
/// - `anyhow::Error` converts via `Into`
/// - `eyre::Report` / `color_eyre::Report` converts via `Into`
/// - Any `std::error::Error + Send + Sync + 'static` converts via `?`
pub type TestError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Result type for integration tests.
pub type TestResult = std::result::Result<(), TestError>;

/// Signature for a plain integration test function.
pub type TestFn = fn() -> TestResult;

/// Signature for a parameterised test (receives one string parameter).
pub type ParameterizedTestFn = fn(&str) -> TestResult;

/// Options that control how a test is dispatched to a VM.
#[derive(Debug, Clone, Default)]
pub struct VmOptions {
    /// Instance type (e.g. `"u1.large"`) passed to `bcvk --itype`.
    ///
    /// When set, takes precedence over `memory_mib` and `vcpus`.
    pub itype: Option<&'static str>,

    /// Explicit VM memory in MiB.
    ///
    /// When `None` and `itype` is also `None`, auto-detected from host
    /// resources (capped at 70% of available memory).
    pub memory_mib: Option<u32>,

    /// Explicit VM vCPU count.
    ///
    /// When `None` and `itype` is also `None`, auto-detected from host
    /// resources.
    pub vcpus: Option<u32>,
}

/// Per-test metadata that maps to external test runner formats.
///
/// This struct captures test properties that are meaningful across
/// runners (tmt, autopkgtest, nextest).  All fields are optional;
/// the harness fills in sensible defaults when emitting metadata.
///
/// Fields are `const`-constructible so they can live in distributed
/// slices.
///
/// # Format mapping
///
/// | Field | tmt (FMF) | autopkgtest (DEP-8) |
/// |---|---|---|
/// | `timeout` | `duration:` | *(global only)* |
/// | `needs_root` | *(plan-level)* | `Restrictions: needs-root` |
/// | `isolation` | *(plan-level)* | `Restrictions: isolation-{container,machine}` |
/// | `tags` | `tag:` | `Classes:` |
/// | `summary` | `summary:` | *(none)* |
/// | `needs_internet` | *(none)* | `Restrictions: needs-internet` |
/// | `flaky` | `result: xfail` | `Restrictions: flaky` |
#[derive(Debug, Clone)]
pub struct TestMeta {
    /// Maximum test duration (e.g. `"5m"`, `"1h"`).
    ///
    /// Defaults to the harness-wide default when `None`.
    pub timeout: Option<&'static str>,

    /// Whether the test requires root privileges.
    ///
    /// Set automatically by [`privileged_test!`] and [`booted_test!`].
    pub needs_root: bool,

    /// Minimum isolation level required.
    ///
    /// Maps to autopkgtest `isolation-container` / `isolation-machine`.
    pub isolation: Isolation,

    /// Free-form tags for filtering and categorization.
    ///
    /// Maps to tmt `tag:` and autopkgtest `Classes:`.
    pub tags: &'static [&'static str],

    /// One-line summary.  Falls back to the test name when `None`.
    pub summary: Option<&'static str>,

    /// Whether the test requires unrestricted internet access.
    pub needs_internet: bool,

    /// Whether the test is known to be flaky.
    ///
    /// Maps to autopkgtest `Restrictions: flaky` and tmt `result: xfail`.
    pub flaky: bool,
}

impl TestMeta {
    /// An empty metadata set — all defaults.
    pub const EMPTY: Self = Self {
        timeout: None,
        needs_root: false,
        isolation: Isolation::None,
        tags: &[],
        summary: None,
        needs_internet: false,
        flaky: false,
    };
}

impl Default for TestMeta {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Minimum isolation level a test requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Isolation {
    /// No special isolation (default).
    #[default]
    None,
    /// Needs its own container (can start services, open ports).
    Container,
    /// Needs its own machine (can interact with kernel, reboot).
    Machine,
}

/// Metadata for a registered integration test.
#[derive(Debug)]
pub struct IntegrationTest {
    /// Name of the test.
    pub name: &'static str,
    /// Test function.
    pub f: TestFn,
    /// Per-test metadata for external runners.
    pub meta: TestMeta,
}

impl IntegrationTest {
    /// Create a new integration test with default metadata.
    pub const fn new(name: &'static str, f: TestFn) -> Self {
        Self {
            name,
            f,
            meta: TestMeta::EMPTY,
        }
    }

    /// Create a new integration test with explicit metadata.
    pub const fn with_meta(name: &'static str, f: TestFn, meta: TestMeta) -> Self {
        Self { name, f, meta }
    }
}

/// Metadata for a parameterised test that is expanded once per parameter value.
#[derive(Debug)]
pub struct ParameterizedIntegrationTest {
    /// Base name (will be suffixed with the parameter value).
    pub name: &'static str,
    /// Test function receiving one string parameter.
    pub f: ParameterizedTestFn,
    /// Per-test metadata for external runners.
    pub meta: TestMeta,
}

impl ParameterizedIntegrationTest {
    /// Create a new parameterised integration test with default metadata.
    pub const fn new(name: &'static str, f: ParameterizedTestFn) -> Self {
        Self {
            name,
            f,
            meta: TestMeta::EMPTY,
        }
    }

    /// Create a new parameterised integration test with explicit metadata.
    pub const fn with_meta(name: &'static str, f: ParameterizedTestFn, meta: TestMeta) -> Self {
        Self { name, f, meta }
    }
}

/// Distributed slice collecting all [`IntegrationTest`]s at link time.
///
/// Used by the [`integration_test!`] macro; not intended for direct use.
#[doc(hidden)]
#[linkme::distributed_slice]
pub static INTEGRATION_TESTS: [IntegrationTest];

/// Distributed slice collecting all [`ParameterizedIntegrationTest`]s.
///
/// Used by the [`parameterized_integration_test!`] macro; not intended
/// for direct use.
#[doc(hidden)]
#[linkme::distributed_slice]
pub static PARAMETERIZED_INTEGRATION_TESTS: [ParameterizedIntegrationTest];

/// Register a test function.
///
/// The function may return any `Result<(), E>` where
/// `E: Into<Box<dyn Error + Send + Sync>>` — this includes
/// `anyhow::Result`, `eyre::Result`, and plain `std::io::Result`.
///
/// ```ignore
/// fn my_test() -> anyhow::Result<()> { Ok(()) }
/// itest::integration_test!(my_test);
/// ```
///
/// With metadata:
///
/// ```ignore
/// fn slow_test() -> itest::TestResult { Ok(()) }
/// itest::integration_test!(slow_test, meta = const { itest::TestMeta {
///     timeout: Some("1h"),
///     tags: &["slow", "network"],
///     needs_internet: true,
///     ..itest::TestMeta::EMPTY
/// }});
/// ```
#[macro_export]
macro_rules! integration_test {
    ($fn_name:ident, meta = const $meta:block) => {
        $crate::paste::paste! {
            // Wrapper converts any compatible error type to TestError.
            fn [<__itest_wrap_ $fn_name>]() -> $crate::TestResult {
                $fn_name().map_err(::std::convert::Into::into)
            }
            #[$crate::linkme::distributed_slice($crate::INTEGRATION_TESTS)]
            static [<__ITEST_ $fn_name:upper>]: $crate::IntegrationTest =
                $crate::IntegrationTest::with_meta(
                    stringify!($fn_name),
                    [<__itest_wrap_ $fn_name>],
                    $meta,
                );
        }
    };
    ($fn_name:ident) => {
        $crate::paste::paste! {
            fn [<__itest_wrap_ $fn_name>]() -> $crate::TestResult {
                $fn_name().map_err(::std::convert::Into::into)
            }
            #[$crate::linkme::distributed_slice($crate::INTEGRATION_TESTS)]
            static [<__ITEST_ $fn_name:upper>]: $crate::IntegrationTest =
                $crate::IntegrationTest::new(
                    stringify!($fn_name),
                    [<__itest_wrap_ $fn_name>],
                );
        }
    };
}

/// Register a parameterised test function.
///
/// The test will be expanded once per parameter value supplied to the harness
/// (e.g. one per container image).
///
/// ```ignore
/// fn my_test(image: &str) -> itest::TestResult { Ok(()) }
/// itest::parameterized_integration_test!(my_test);
/// ```
///
/// With metadata:
///
/// ```ignore
/// fn slow_test(image: &str) -> itest::TestResult { Ok(()) }
/// itest::parameterized_integration_test!(slow_test, meta = const { itest::TestMeta {
///     timeout: Some("30m"),
///     ..itest::TestMeta::EMPTY
/// }});
/// ```
#[macro_export]
macro_rules! parameterized_integration_test {
    ($fn_name:ident, meta = const $meta:block) => {
        $crate::paste::paste! {
            fn [<__itest_wrap_ $fn_name>](p: &str) -> $crate::TestResult {
                $fn_name(p).map_err(::std::convert::Into::into)
            }
            #[$crate::linkme::distributed_slice($crate::PARAMETERIZED_INTEGRATION_TESTS)]
            static [<__ITEST_ $fn_name:upper>]: $crate::ParameterizedIntegrationTest =
                $crate::ParameterizedIntegrationTest::with_meta(
                    stringify!($fn_name),
                    [<__itest_wrap_ $fn_name>],
                    $meta,
                );
        }
    };
    ($fn_name:ident) => {
        $crate::paste::paste! {
            fn [<__itest_wrap_ $fn_name>](p: &str) -> $crate::TestResult {
                $fn_name(p).map_err(::std::convert::Into::into)
            }
            #[$crate::linkme::distributed_slice($crate::PARAMETERIZED_INTEGRATION_TESTS)]
            static [<__ITEST_ $fn_name:upper>]: $crate::ParameterizedIntegrationTest =
                $crate::ParameterizedIntegrationTest::new(
                    stringify!($fn_name),
                    [<__itest_wrap_ $fn_name>],
                );
        }
    };
}

/// Create a test that requires root privileges.
///
/// When not running as root the test is automatically dispatched inside a
/// bcvk ephemeral VM (fast path, no disk install).
///
/// The test binary name is taken from the first argument; it must match the
/// installed binary name so that `bcvk ephemeral run-ssh` can invoke it.
///
/// An optional `itype = "..."` argument specifies the VM instance type
/// (e.g. `"u1.large"` for 2 vCPU / 8 GiB).  When omitted the default
/// instance type is used.
///
/// ```ignore
/// itest::privileged_test!("my-binary", my_test, {
///     // runs as root with default VM size
///     Ok(())
/// });
///
/// itest::privileged_test!("my-binary", big_test, itype = "u1.large", {
///     // runs as root in a larger VM — note trailing comma after itype
///     Ok(())
/// });
/// ```
#[macro_export]
macro_rules! privileged_test {
    ($binary:expr, $fn_name:ident, $(itype = $itype:expr,)? $body:expr) => {
        fn $fn_name() -> $crate::TestResult {
            #[allow(unused_mut)]
            let mut vm_opts = $crate::VmOptions::default();
            $( vm_opts.itype = Some($itype); )?
            if $crate::require_root(
                stringify!($fn_name),
                $binary,
                $crate::DispatchMode::Privileged,
                &vm_opts,
            )?
            .is_some()
            {
                return Ok(());
            }
            // Inner closure: return type is inferred from $body,
            // allowing any Result<(), E> where E: Into<TestError>.
            let inner = || $body;
            inner().map_err(::std::convert::Into::into)
        }
        $crate::integration_test!($fn_name, meta = const {
            $crate::TestMeta {
                needs_root: true,
                ..$crate::TestMeta::EMPTY
            }
        });
    };
}

/// Create a test that requires a fully booted (e.g. ostree-deployed) system.
///
/// When not running as root the test is dispatched via `bcvk libvirt run`
/// which does a full `bootc install to-disk`.
///
/// An optional `itype = "..."` argument specifies the VM instance type.
///
/// ```ignore
/// itest::booted_test!("my-binary", my_test, {
///     // runs inside a booted ostree deployment
///     Ok(())
/// });
///
/// itest::booted_test!("my-binary", big_test, itype = "u1.large", {
///     // runs in a larger VM — note trailing comma after itype
///     Ok(())
/// });
/// ```
#[macro_export]
macro_rules! booted_test {
    ($binary:expr, $fn_name:ident, $(itype = $itype:expr,)? $body:expr) => {
        fn $fn_name() -> $crate::TestResult {
            #[allow(unused_mut)]
            let mut vm_opts = $crate::VmOptions::default();
            $( vm_opts.itype = Some($itype); )?
            if $crate::require_root(
                stringify!($fn_name),
                $binary,
                $crate::DispatchMode::Booted,
                &vm_opts,
            )?
            .is_some()
            {
                return Ok(());
            }
            let inner = || $body;
            inner().map_err(::std::convert::Into::into)
        }
        $crate::integration_test!($fn_name, meta = const {
            $crate::TestMeta {
                needs_root: true,
                isolation: $crate::Isolation::Machine,
                ..$crate::TestMeta::EMPTY
            }
        });
    };
}

/// Replace non-alphanumeric characters with underscores.
///
/// Useful for turning container image references into safe test-name suffixes.
pub fn image_to_test_suffix(image: &str) -> String {
    image.replace(|c: char| !c.is_alphanumeric(), "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_basic() {
        assert_eq!(
            image_to_test_suffix("quay.io/fedora/fedora-bootc:42"),
            "quay_io_fedora_fedora_bootc_42"
        );
    }

    #[test]
    fn suffix_digest() {
        assert_eq!(
            image_to_test_suffix("quay.io/image@sha256:abc123"),
            "quay_io_image_sha256_abc123"
        );
    }

    #[test]
    fn suffix_only_alnum() {
        assert_eq!(image_to_test_suffix("simpleimage"), "simpleimage");
    }
}
