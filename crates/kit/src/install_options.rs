//! Common installation options shared across bcvk commands
//!
//! This module provides shared configuration structures for disk installation
//! operations, ensuring consistency across to-disk, libvirt-upload-disk,
//! and other installation-related commands.

// On non-Linux, this module is unused as it's for installation operations
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use camino::Utf8PathBuf;
use clap::Parser;

/// Common installation options for bootc disk operations
///
/// These options control filesystem configuration and storage paths
/// for bootc installation commands. Use `#[clap(flatten)]` to include
/// these in command-specific option structures.
#[derive(Debug, Default, Parser, Clone)]
pub struct InstallOptions {
    /// Root filesystem type (overrides bootc image default)
    #[clap(long, help = "Root filesystem type (e.g. ext4, xfs, btrfs)")]
    pub filesystem: Option<String>,

    /// Custom root filesystem size (e.g., '10G', '5120M')
    #[clap(long, help = "Root filesystem size (e.g., '10G', '5120M')")]
    pub root_size: Option<String>,

    /// Path to host container storage (auto-detected if not specified)
    #[clap(
        long,
        help = "Path to host container storage (auto-detected if not specified)"
    )]
    pub storage_path: Option<Utf8PathBuf>,

    /// The transport; e.g. oci, oci-archive, containers-storage.  Defaults to `registry`
    #[clap(long)]
    pub target_transport: Option<String>,

    #[clap(long)]
    /// Set a kernel argument
    pub karg: Vec<String>,

    /// Default to composefs-native storage
    #[clap(long)]
    pub composefs_backend: bool,

    /// Which bootloader to use for composefs-native backend
    #[clap(long, requires = "composefs_backend")]
    pub bootloader: Option<String>,

    /// Allow installation without fs-verity support for composefs-native
    /// backend
    #[clap(long, requires = "composefs_backend")]
    pub allow_missing_fsverity: bool,
}

impl InstallOptions {
    /// Get the bootc install command arguments for these options
    pub fn to_bootc_args(&self) -> Vec<String> {
        let mut args = vec![];

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

        if let Some(ref t) = self.target_transport {
            args.push("--target-transport".to_string());
            args.push(t.clone());
        }

        if self.composefs_backend {
            args.push("--composefs-backend".to_owned());
        }

        if let Some(b) = &self.bootloader {
            args.push("--bootloader".into());
            args.push(b.clone());
        }

        if self.allow_missing_fsverity {
            args.push("--allow-missing-fsverity".into());
        }

        args
    }
}
