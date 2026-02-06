//! Anaconda integration for bcvk
//!
//! This module provides integration with the Anaconda installer to enable
//! bootc container installation using anaconda's capabilities for hardware
//! detection, partitioning, and system configuration via kickstart files.

use clap::Subcommand;

pub mod install;

#[derive(Debug, Subcommand)]
pub enum AnacondaSubcommands {
    /// Install a bootc container using anaconda
    Install(install::AnacondaInstallOpts),
}

#[derive(Debug, Clone, Default)]
pub struct AnacondaOptions {}
