//! Shared library code for integration tests
//!
//! This module contains constants and utilities that are shared between
//! the main test binary and helper binaries like cleanup.

// Unfortunately needed here to work with linkme
#![allow(unsafe_code)]

/// Label used to identify containers created by integration tests
pub const INTEGRATION_TEST_LABEL: &str = "bcvk.integration-test=1";

/// Label used to identify libvirt VMs created by integration tests
pub const LIBVIRT_INTEGRATION_TEST_LABEL: &str = "bcvk-integration";

/// A test function that returns a Result
pub type TestFn = fn() -> color_eyre::Result<()>;

/// A parameterized test function that takes an image parameter
pub type ParameterizedTestFn = fn(&str) -> color_eyre::Result<()>;

/// Metadata for a registered integration test
#[derive(Debug)]
pub struct IntegrationTest {
    /// Name of the integration test
    pub name: &'static str,
    /// Test function to execute
    pub f: TestFn,
}

impl IntegrationTest {
    /// Create a new integration test with the given name and function
    pub const fn new(name: &'static str, f: TestFn) -> Self {
        Self { name, f }
    }
}

/// Metadata for a parameterized integration test that runs once per image
#[derive(Debug)]
pub struct ParameterizedIntegrationTest {
    /// Base name of the integration test (will be suffixed with image identifier)
    pub name: &'static str,
    /// Parameterized test function to execute
    pub f: ParameterizedTestFn,
}

impl ParameterizedIntegrationTest {
    /// Create a new parameterized integration test with the given name and function
    pub const fn new(name: &'static str, f: ParameterizedTestFn) -> Self {
        Self { name, f }
    }
}

/// Distributed slice holding all registered integration tests
#[linkme::distributed_slice]
pub static INTEGRATION_TESTS: [IntegrationTest];

/// Distributed slice holding all registered parameterized integration tests
#[linkme::distributed_slice]
pub static PARAMETERIZED_INTEGRATION_TESTS: [ParameterizedIntegrationTest];

/// Register an integration test with less boilerplate.
///
/// This macro generates the static registration for an integration test function.
///
/// # Examples
///
/// ```ignore
/// fn test_basic_functionality() -> Result<()> {
///     let sh = shell()?;
///     let bck = get_bck_command()?;
///     cmd!(sh, "{bck} some args").run()?;
///     Ok(())
/// }
/// integration_test!(test_basic_functionality);
/// ```
#[macro_export]
macro_rules! integration_test {
    ($fn_name:ident) => {
        ::paste::paste! {
            #[::linkme::distributed_slice($crate::INTEGRATION_TESTS)]
            static [<$fn_name:upper>]: $crate::IntegrationTest =
                $crate::IntegrationTest::new(stringify!($fn_name), $fn_name);
        }
    };
}

/// Register a parameterized integration test with less boilerplate.
///
/// This macro generates the static registration for a parameterized integration test function.
///
/// # Examples
///
/// ```ignore
/// fn test_with_image(image: &str) -> Result<()> {
///     let sh = shell()?;
///     let bck = get_bck_command()?;
///     cmd!(sh, "{bck} command {image}").run()?;
///     Ok(())
/// }
/// parameterized_integration_test!(test_with_image);
/// ```
#[macro_export]
macro_rules! parameterized_integration_test {
    ($fn_name:ident) => {
        ::paste::paste! {
            #[::linkme::distributed_slice($crate::PARAMETERIZED_INTEGRATION_TESTS)]
            static [<$fn_name:upper>]: $crate::ParameterizedIntegrationTest =
                $crate::ParameterizedIntegrationTest::new(stringify!($fn_name), $fn_name);
        }
    };
}

/// Create a test suffix from an image name by replacing invalid characters with underscores
///
/// Replaces all non-alphanumeric characters with `_` to create a predictable, filesystem-safe
/// test name suffix.
///
/// Examples:
/// - "quay.io/fedora/fedora-bootc:42" -> "quay_io_fedora_fedora_bootc_42"
/// - "quay.io/centos-bootc/centos-bootc:stream10" -> "quay_io_centos_bootc_centos_bootc_stream10"
/// - "quay.io/image@sha256:abc123" -> "quay_io_image_sha256_abc123"
pub fn image_to_test_suffix(image: &str) -> String {
    image.replace(|c: char| !c.is_alphanumeric(), "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_to_test_suffix_basic() {
        assert_eq!(
            image_to_test_suffix("quay.io/fedora/fedora-bootc:42"),
            "quay_io_fedora_fedora_bootc_42"
        );
    }

    #[test]
    fn test_image_to_test_suffix_stream() {
        assert_eq!(
            image_to_test_suffix("quay.io/centos-bootc/centos-bootc:stream10"),
            "quay_io_centos_bootc_centos_bootc_stream10"
        );
    }

    #[test]
    fn test_image_to_test_suffix_digest() {
        assert_eq!(
            image_to_test_suffix("quay.io/image@sha256:abc123"),
            "quay_io_image_sha256_abc123"
        );
    }

    #[test]
    fn test_image_to_test_suffix_complex() {
        assert_eq!(
            image_to_test_suffix("registry.example.com:5000/my-org/my-image:v1.2.3"),
            "registry_example_com_5000_my_org_my_image_v1_2_3"
        );
    }

    #[test]
    fn test_image_to_test_suffix_only_alphanumeric() {
        assert_eq!(image_to_test_suffix("simpleimage"), "simpleimage");
    }

    #[test]
    fn test_image_to_test_suffix_special_chars() {
        assert_eq!(
            image_to_test_suffix("image/with@special:chars-here.now"),
            "image_with_special_chars_here_now"
        );
    }
}
