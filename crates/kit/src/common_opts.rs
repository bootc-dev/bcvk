//! Common CLI options shared across commands

use clap::Parser;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const DEFAULT_MEMORY_USER_STR: &str = "4G";
pub const DEFAULT_CPUS: u32 = 2;
pub const DEFAULT_DISK_SIZE: &str = "20G";
pub const DEFAULT_NETWORK: &str = "user";

/// Memory size options
#[derive(Parser, Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryOpts {
    #[clap(
        long,
        default_value = DEFAULT_MEMORY_USER_STR,
        help = "Memory size (e.g. 4G, 2048M, or plain number for MB)"
    )]
    #[serde(default = "default_memory")]
    pub memory: String,
}

fn default_memory() -> String {
    DEFAULT_MEMORY_USER_STR.to_string()
}

impl fmt::Display for MemoryOpts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.memory)
    }
}

/// CPU count options
#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
pub struct CpuOpts {
    #[clap(long, default_value_t = DEFAULT_CPUS, help = "Number of virtual CPUs")]
    #[serde(default = "default_cpus")]
    pub cpus: u32,
}

impl Default for CpuOpts {
    fn default() -> Self {
        Self { cpus: DEFAULT_CPUS }
    }
}

fn default_cpus() -> u32 {
    DEFAULT_CPUS
}

/// Disk size options
#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
pub struct DiskSizeOpts {
    #[clap(
        long,
        default_value = DEFAULT_DISK_SIZE,
        help = "Disk size (e.g. 20G, 10240M, or plain number for bytes)"
    )]
    #[serde(default = "default_disk_size", rename = "disk-size")]
    pub disk_size: String,
}

impl Default for DiskSizeOpts {
    fn default() -> Self {
        Self {
            disk_size: DEFAULT_DISK_SIZE.to_string(),
        }
    }
}

fn default_disk_size() -> String {
    DEFAULT_DISK_SIZE.to_string()
}

/// Network mode options
#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
pub struct NetworkOpts {
    #[clap(long, default_value = DEFAULT_NETWORK, help = "Network mode for the VM")]
    #[serde(default = "default_network")]
    pub network: String,
}

impl Default for NetworkOpts {
    fn default() -> Self {
        Self {
            network: DEFAULT_NETWORK.to_string(),
        }
    }
}

fn default_network() -> String {
    DEFAULT_NETWORK.to_string()
}
