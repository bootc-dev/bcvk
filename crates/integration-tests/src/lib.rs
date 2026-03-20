//! Shared library code for bcvk integration tests.
//!
//! Re-exports the core test infrastructure from [`itest`] and adds
//! bcvk-specific constants.

// linkme (via itest) requires unsafe for distributed slices
#![allow(unsafe_code)]

// Re-export everything consumers need from itest so that test modules
// can continue to write `use integration_tests::integration_test;`.
pub use itest::image_to_test_suffix;
pub use itest::integration_test;
pub use itest::parameterized_integration_test;
pub use itest::IntegrationTest;
pub use itest::ParameterizedIntegrationTest;
pub use itest::TestFn;
pub use itest::INTEGRATION_TESTS;
pub use itest::PARAMETERIZED_INTEGRATION_TESTS;

/// Label used to identify containers created by integration tests.
pub const INTEGRATION_TEST_LABEL: &str = "bcvk.integration-test=1";

/// Label used to identify libvirt VMs created by integration tests.
pub const LIBVIRT_INTEGRATION_TEST_LABEL: &str = "bcvk-integration";
