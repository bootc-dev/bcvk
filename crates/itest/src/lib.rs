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

pub use harness::{run_tests, run_tests_with_config, TestConfig};
pub use privilege::{require_root, DispatchMode};

// Re-export dependencies used by our macros so consumers don't need
// to add them to their own Cargo.toml.
#[doc(hidden)]
pub use anyhow;
#[doc(hidden)]
pub use linkme;
#[doc(hidden)]
pub use paste;

/// Signature for a plain integration test function.
pub type TestFn = fn() -> anyhow::Result<()>;

/// Signature for a parameterised test (receives one string parameter).
pub type ParameterizedTestFn = fn(&str) -> anyhow::Result<()>;

/// Metadata for a registered integration test.
#[derive(Debug)]
pub struct IntegrationTest {
    /// Name of the test.
    pub name: &'static str,
    /// Test function.
    pub f: TestFn,
}

impl IntegrationTest {
    /// Create a new integration test.
    pub const fn new(name: &'static str, f: TestFn) -> Self {
        Self { name, f }
    }
}

/// Metadata for a parameterised test that is expanded once per parameter value.
#[derive(Debug)]
pub struct ParameterizedIntegrationTest {
    /// Base name (will be suffixed with the parameter value).
    pub name: &'static str,
    /// Test function receiving one string parameter.
    pub f: ParameterizedTestFn,
}

impl ParameterizedIntegrationTest {
    /// Create a new parameterised integration test.
    pub const fn new(name: &'static str, f: ParameterizedTestFn) -> Self {
        Self { name, f }
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
/// ```ignore
/// fn my_test() -> anyhow::Result<()> { Ok(()) }
/// itest::integration_test!(my_test);
/// ```
#[macro_export]
macro_rules! integration_test {
    ($fn_name:ident) => {
        $crate::paste::paste! {
            #[$crate::linkme::distributed_slice($crate::INTEGRATION_TESTS)]
            static [<$fn_name:upper>]: $crate::IntegrationTest =
                $crate::IntegrationTest::new(stringify!($fn_name), $fn_name);
        }
    };
}

/// Register a parameterised test function.
///
/// The test will be expanded once per parameter value supplied to the harness
/// (e.g. one per container image).
///
/// ```ignore
/// fn my_test(image: &str) -> anyhow::Result<()> { Ok(()) }
/// itest::parameterized_integration_test!(my_test);
/// ```
#[macro_export]
macro_rules! parameterized_integration_test {
    ($fn_name:ident) => {
        $crate::paste::paste! {
            #[$crate::linkme::distributed_slice($crate::PARAMETERIZED_INTEGRATION_TESTS)]
            static [<$fn_name:upper>]: $crate::ParameterizedIntegrationTest =
                $crate::ParameterizedIntegrationTest::new(stringify!($fn_name), $fn_name);
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
/// ```ignore
/// itest::privileged_test!("my-binary", my_test, {
///     // runs as root
///     Ok(())
/// });
/// ```
#[macro_export]
macro_rules! privileged_test {
    ($binary:expr, $fn_name:ident, $body:expr) => {
        fn $fn_name() -> $crate::anyhow::Result<()> {
            if $crate::require_root(
                stringify!($fn_name),
                $binary,
                $crate::DispatchMode::Privileged,
            )?
            .is_some()
            {
                return Ok(());
            }
            $body
        }
        $crate::integration_test!($fn_name);
    };
}

/// Create a test that requires a fully booted (e.g. ostree-deployed) system.
///
/// When not running as root the test is dispatched via `bcvk libvirt run`
/// which does a full `bootc install to-disk`.
///
/// ```ignore
/// itest::booted_test!("my-binary", my_test, {
///     // runs inside a booted ostree deployment
///     Ok(())
/// });
/// ```
#[macro_export]
macro_rules! booted_test {
    ($binary:expr, $fn_name:ident, $body:expr) => {
        fn $fn_name() -> $crate::anyhow::Result<()> {
            if $crate::require_root(stringify!($fn_name), $binary, $crate::DispatchMode::Booted)?
                .is_some()
            {
                return Ok(());
            }
            $body
        }
        $crate::integration_test!($fn_name);
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
