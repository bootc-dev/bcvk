//! KubeVirt common-instancetypes support
//!
//! This module vendors the KubeVirt common-instancetypes definitions,
//! specifically the U series (Universal/General Purpose) instance types.
//! These provide standardized VM sizing with predefined vCPU and memory
//! configurations.
//!
//! Instance types follow the format: u1.{size}
//! Examples: u1.nano, u1.micro, u1.small, u1.medium, u1.large, etc.
//!
//! Source: https://github.com/kubevirt/common-instancetypes

// On non-Linux, this module is unused as it's for VM instance types
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

/// Instance type variants with associated vCPU and memory specifications
///
/// Source: https://github.com/kubevirt/common-instancetypes/blob/main/instancetypes/u/1/sizes.yaml
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    strum::Display,
    strum::EnumString,
    strum::EnumIter,
)]
#[non_exhaustive]
pub enum InstanceType {
    /// u1.nano - 1 vCPU, 512 MiB memory
    #[strum(serialize = "u1.nano")]
    U1Nano,
    /// u1.micro - 1 vCPU, 1 GiB memory
    #[strum(serialize = "u1.micro")]
    U1Micro,
    /// u1.small - 1 vCPU, 2 GiB memory
    #[strum(serialize = "u1.small")]
    U1Small,
    /// u1.medium - 1 vCPU, 4 GiB memory
    #[strum(serialize = "u1.medium")]
    U1Medium,
    /// u1.2xmedium - 2 vCPU, 4 GiB memory
    #[strum(serialize = "u1.2xmedium")]
    U1TwoXMedium,
    /// u1.large - 2 vCPU, 8 GiB memory
    #[strum(serialize = "u1.large")]
    U1Large,
    /// u1.xlarge - 4 vCPU, 16 GiB memory
    #[strum(serialize = "u1.xlarge")]
    U1XLarge,
    /// u1.2xlarge - 8 vCPU, 32 GiB memory
    #[strum(serialize = "u1.2xlarge")]
    U1TwoXLarge,
    /// u1.4xlarge - 16 vCPU, 64 GiB memory
    #[strum(serialize = "u1.4xlarge")]
    U1FourXLarge,
    /// u1.8xlarge - 32 vCPU, 128 GiB memory
    #[strum(serialize = "u1.8xlarge")]
    U1EightXLarge,
}

impl InstanceType {
    /// Get the number of vCPUs for this instance type
    pub const fn vcpus(self) -> u32 {
        match self {
            Self::U1Nano => 1,
            Self::U1Micro => 1,
            Self::U1Small => 1,
            Self::U1Medium => 1,
            Self::U1TwoXMedium => 2,
            Self::U1Large => 2,
            Self::U1XLarge => 4,
            Self::U1TwoXLarge => 8,
            Self::U1FourXLarge => 16,
            Self::U1EightXLarge => 32,
        }
    }

    /// Get the memory in megabytes for this instance type
    pub const fn memory_mb(self) -> u32 {
        match self {
            Self::U1Nano => 512,
            Self::U1Micro => 1024,
            Self::U1Small => 2048,
            Self::U1Medium => 4096,
            Self::U1TwoXMedium => 4096,
            Self::U1Large => 8192,
            Self::U1XLarge => 16384,
            Self::U1TwoXLarge => 32768,
            Self::U1FourXLarge => 65536,
            Self::U1EightXLarge => 131072,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use strum::IntoEnumIterator;

    #[test]
    fn test_properties() {
        for variant in InstanceType::iter() {
            let (expected_vcpus, expected_memory_mb) = match variant {
                InstanceType::U1Nano => (1, 512),
                InstanceType::U1Micro => (1, 1024),
                InstanceType::U1Small => (1, 2048),
                InstanceType::U1Medium => (1, 4096),
                InstanceType::U1TwoXMedium => (2, 4096),
                InstanceType::U1Large => (2, 8192),
                InstanceType::U1XLarge => (4, 16384),
                InstanceType::U1TwoXLarge => (8, 32768),
                InstanceType::U1FourXLarge => (16, 65536),
                InstanceType::U1EightXLarge => (32, 131072),
            };
            assert_eq!(
                variant.vcpus(),
                expected_vcpus,
                "Mismatch in vcpus for {:?}",
                variant
            );
            assert_eq!(
                variant.memory_mb(),
                expected_memory_mb,
                "Mismatch in memory_mb for {:?}",
                variant
            );
        }
    }

    #[test]
    fn test_parse_invalid_instancetype() {
        let result = InstanceType::from_str("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_roundtrip() {
        for variant in InstanceType::iter() {
            let s = variant.to_string();
            let parsed = InstanceType::from_str(&s).unwrap();
            assert_eq!(parsed, variant);
        }
    }
}
