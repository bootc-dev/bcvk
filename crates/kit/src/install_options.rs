//! Common installation options shared across bcvk commands
//!
//! This module provides shared configuration structures for disk installation
//! operations, ensuring consistency across to-disk, libvirt-upload-disk,
//! and other installation-related commands.

use camino::Utf8PathBuf;
use clap::Parser;
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Common installation options for bootc disk operations
///
/// These options control filesystem configuration and storage paths
/// for bootc installation commands. Use `#[clap(flatten)]` to include
/// these in command-specific option structures.
#[derive(Debug, Default, Parser, Clone, Serialize)]
pub struct InstallOptions {
    /// Root filesystem type (overrides bootc image default)
    #[clap(long, help = "Root filesystem type (e.g. ext4, xfs, btrfs)")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filesystem: Option<String>,

    /// Custom root filesystem size (e.g., '10G', '5120M')
    #[clap(long, help = "Root filesystem size (e.g., '10G', '5120M')")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_size: Option<String>,

    /// Path to host container storage (auto-detected if not specified)
    /// NOTE: This does NOT affect the generated disk content, only where to find the source image
    #[clap(
        long,
        help = "Path to host container storage (auto-detected if not specified)"
    )]
    #[serde(skip)]
    pub storage_path: Option<Utf8PathBuf>,

    #[clap(long)]
    /// Set a kernel argument
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub karg: Vec<String>,

    /// Default to composefs-native storage
    #[clap(long)]
    pub composefs_native: bool,

    /// Target transport for image pulling (e.g., "containers-storage")
    /// Not exposed via CLI - set programmatically when needed
    #[clap(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_transport: Option<String>,
}

/// Internal structure for computing cache hash
/// Wraps InstallOptions with additional metadata
#[derive(Serialize)]
struct CacheHashInputs<'a> {
    /// SHA256 digest of the source container image
    image_digest: &'a str,

    /// All install options that affect the generated disk
    /// (storage_path is excluded via #[serde(skip)])
    #[serde(flatten)]
    options: &'a InstallOptions,

    /// Version of the cache format for future compatibility
    version: u32,
}

impl InstallOptions {
    /// Compute SHA256 hash of all inputs that affect the generated disk
    ///
    /// This hash is used for cache validation. Any new field added to InstallOptions
    /// will automatically affect the hash (unless marked with #[serde(skip)] or
    /// #[serde(skip_serializing_if)]).
    ///
    /// Fields excluded from hash:
    /// - storage_path: Only affects where to find the source image, not disk content
    /// - Option<T> fields when None: Skipped to maintain hash stability
    pub fn compute_hash(&self, image_digest: &str) -> String {
        let inputs = CacheHashInputs {
            image_digest,
            options: self,
            version: 1,
        };

        let json = serde_json::to_string(&inputs).expect("Failed to serialize cache inputs");
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        format!("sha256:{:x}", hasher.finalize())
    }

    /// Get the bootc install command arguments for these options
    pub fn to_bootc_args(&self) -> Vec<String> {
        let mut args = vec![];

        if let Some(ref target_transport) = self.target_transport {
            args.push("--target-transport".to_string());
            args.push(target_transport.clone());
        }

        if let Some(ref filesystem) = self.filesystem {
            args.push("--filesystem".to_string());
            args.push(filesystem.clone());
        }

        if let Some(ref root_size) = self.root_size {
            args.push("--root-size".to_string());
            args.push(root_size.clone());
        }

        for k in self.karg.iter() {
            args.push(format!("--karg={k}"));
        }

        if self.composefs_native {
            args.push("--composefs-native".to_owned());
        }

        args
    }
}
