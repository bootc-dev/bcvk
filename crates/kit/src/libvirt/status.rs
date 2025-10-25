//! libvirt status command - show libvirt environment information
//!
//! This module provides a status command that outputs JSON metadata about
//! the libvirt environment, including version information and domain count.

use clap::Parser;
use color_eyre::{eyre::Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::sync::OnceLock;

use crate::domain_list::DomainLister;

/// Options for the libvirt status command
#[derive(Debug, Parser)]
pub struct LibvirtStatusOpts {
    /// Output format (yaml or json)
    #[clap(long, default_value = "yaml", value_enum)]
    pub format: OutputFormat,
}

/// Output format for status command
#[derive(Debug, Clone, clap::ValueEnum)]
pub enum OutputFormat {
    /// YAML format (default, human-readable)
    Yaml,
    /// JSON format (machine-readable)
    Json,
}

/// libvirt version information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibvirtVersion {
    pub major: u32,
    pub minor: u32,
    pub micro: u32,
    pub full_version: String,
}

/// libvirt status information
#[derive(Debug, Serialize, Deserialize)]
pub struct LibvirtStatus {
    pub version: Option<LibvirtVersion>,
    pub supports_readonly_virtiofs: bool,
    pub domain_count: usize,
    pub running_domain_count: usize,
}

/// Parse a version string like "6.2.0" into LibvirtVersion struct
fn parse_version_string(version_str: &str) -> Option<LibvirtVersion> {
    let parts: Vec<&str> = version_str.split('.').collect();
    if parts.is_empty() {
        return None;
    }

    let major = parts[0].parse::<u32>().ok()?;
    let minor = if parts.len() > 1 {
        parts[1].parse::<u32>().unwrap_or(0)
    } else {
        0
    };
    let micro = if parts.len() > 2 {
        parts[2].parse::<u32>().unwrap_or(0)
    } else {
        0
    };

    Some(LibvirtVersion {
        major,
        minor,
        micro,
        full_version: version_str.to_string(),
    })
}

/// Parse libvirt version from virsh version output text
fn parse_libvirt_version_from_output(version_output: &str) -> Option<LibvirtVersion> {
    // Parse version from output like "Compiled against library: libvirt 6.2.0"
    // or "Using library: libvirt 6.2.0"
    for line in version_output.lines() {
        if line.contains("libvirt") {
            // Find "libvirt X.Y.Z" pattern
            if let Some(version_part) = line.strip_prefix("libvirt ").or_else(|| {
                line.find("libvirt ")
                    .and_then(|start| line[start..].strip_prefix("libvirt "))
            }) {
                let version_end = version_part.find(' ').unwrap_or(version_part.len());
                let version_str = &version_part[..version_end];

                if let Some(version) = parse_version_string(version_str) {
                    return Some(version);
                }
            }
        }
    }

    None
}

/// Parse libvirt version from virsh version output
fn parse_libvirt_version_uncached() -> Result<Option<LibvirtVersion>> {
    let output = Command::new("virsh")
        .args(&["version"])
        .output()
        .with_context(|| "Failed to check libvirt version")?;

    if !output.status.success() {
        return Ok(None);
    }

    let version_output = String::from_utf8(output.stdout)
        .with_context(|| "virsh version output contained invalid UTF-8")?;

    Ok(parse_libvirt_version_from_output(&version_output))
}

/// Cached libvirt version (parsed once per process)
static LIBVIRT_VERSION: OnceLock<Option<LibvirtVersion>> = OnceLock::new();

/// Get the cached libvirt version, parsing it on first call
pub fn parse_libvirt_version() -> Result<Option<LibvirtVersion>> {
    // If already cached, clone and return
    if let Some(version) = LIBVIRT_VERSION.get() {
        return Ok(version.clone());
    }

    // Parse version and cache it
    let version = parse_libvirt_version_uncached()?;
    let _ = LIBVIRT_VERSION.set(version.clone());
    Ok(version)
}

/// Check if libvirt supports readonly virtiofs
pub fn supports_readonly_virtiofs(version: &Option<LibvirtVersion>) -> bool {
    match version {
        Some(v) => {
            // Requires libvirt 11.0+ for readonly virtiofs support
            v.major >= 11
        }
        None => false,
    }
}

/// Execute the libvirt status command
pub fn run(opts: LibvirtStatusOpts) -> Result<()> {
    // Get libvirt version
    let version = parse_libvirt_version()?;
    let supports_readonly = supports_readonly_virtiofs(&version);

    // Get domain count
    let lister = DomainLister::new();
    let all_domains = lister
        .list_all_domains()
        .with_context(|| "Failed to list domains")?;

    // Count running domains by checking state
    let mut running_count = 0;
    for domain_name in &all_domains {
        if let Ok(state) = lister.get_domain_state(domain_name) {
            if state == "running" {
                running_count += 1;
            }
        }
    }

    let status = LibvirtStatus {
        version,
        supports_readonly_virtiofs: supports_readonly,
        domain_count: all_domains.len(),
        running_domain_count: running_count,
    };

    // Output in requested format
    match opts.format {
        OutputFormat::Yaml => {
            println!(
                "{}",
                serde_yaml::to_string(&status)
                    .with_context(|| "Failed to serialize status as YAML")?
            );
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&status)
                    .with_context(|| "Failed to serialize status as JSON")?
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version_string() {
        // Test full version string
        let version = parse_version_string("6.2.0").unwrap();
        assert_eq!(version.major, 6);
        assert_eq!(version.minor, 2);
        assert_eq!(version.micro, 0);
        assert_eq!(version.full_version, "6.2.0");

        // Test major.minor only
        let version = parse_version_string("11.5").unwrap();
        assert_eq!(version.major, 11);
        assert_eq!(version.minor, 5);
        assert_eq!(version.micro, 0);
        assert_eq!(version.full_version, "11.5");

        // Test major only
        let version = parse_version_string("12").unwrap();
        assert_eq!(version.major, 12);
        assert_eq!(version.minor, 0);
        assert_eq!(version.micro, 0);
        assert_eq!(version.full_version, "12");

        // Test invalid version strings
        assert!(parse_version_string("").is_none());
        assert!(parse_version_string("not_a_number").is_none());

        // Test version with non-numeric minor version - should work with fallback to 0
        let version = parse_version_string("6.x.0").unwrap();
        assert_eq!(version.major, 6);
        assert_eq!(version.minor, 0); // fallback to 0 for non-numeric

        // Test version with additional parts (should ignore them)
        let version = parse_version_string("6.2.0.1").unwrap();
        assert_eq!(version.major, 6);
        assert_eq!(version.minor, 2);
        assert_eq!(version.micro, 0);
        assert_eq!(version.full_version, "6.2.0.1");
    }

    #[test]
    fn test_parse_libvirt_version_from_output() {
        // Test typical virsh version output
        let output = "Compiled against library: libvirt 6.2.0\nUsing library: libvirt 6.2.0\n";
        let version = parse_libvirt_version_from_output(output).unwrap();
        assert_eq!(version.major, 6);
        assert_eq!(version.minor, 2);
        assert_eq!(version.micro, 0);

        // Test with different format
        let output = "libvirt 11.0.0\n";
        let version = parse_libvirt_version_from_output(output).unwrap();
        assert_eq!(version.major, 11);
        assert_eq!(version.minor, 0);
        assert_eq!(version.micro, 0);

        // Test with no libvirt version
        let output = "Some other output without version\n";
        assert!(parse_libvirt_version_from_output(output).is_none());

        // Test with libvirt mentioned but no version
        let output = "libvirt is installed\n";
        assert!(parse_libvirt_version_from_output(output).is_none());
    }

    #[test]
    fn test_supports_readonly_virtiofs() {
        // Test version that supports readonly virtiofs
        let version = Some(LibvirtVersion {
            major: 11,
            minor: 0,
            micro: 0,
            full_version: "11.0.0".to_string(),
        });
        assert!(supports_readonly_virtiofs(&version));

        // Test version that doesn't support readonly virtiofs
        let version = Some(LibvirtVersion {
            major: 10,
            minor: 5,
            micro: 0,
            full_version: "10.5.0".to_string(),
        });
        assert!(!supports_readonly_virtiofs(&version));

        // Test with no version
        assert!(!supports_readonly_virtiofs(&None));

        // Test edge case - exactly version 11.0.0
        let version = Some(LibvirtVersion {
            major: 11,
            minor: 0,
            micro: 0,
            full_version: "11.0.0".to_string(),
        });
        assert!(supports_readonly_virtiofs(&version));
    }
}
