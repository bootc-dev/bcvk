//! Systemd credential injection for bootc VMs.
//!
//! This module re-exports credential types and functions from the bcvk-qemu crate.
//! Provides functions for injecting configuration into VMs via systemd credentials
//! using SMBIOS firmware variables (preferred) or kernel command-line arguments.
//! Supports SSH keys, mount units, environment configuration, and AF_VSOCK setup.

// Re-export credential functions from bcvk-qemu that are used within kit
pub use bcvk_qemu::{
    generate_virtiofs_mount_unit, guest_path_to_unit_name, key_to_root_tmpfiles_d,
    smbios_cred_for_root_ssh, smbios_creds_for_storage_opts, storage_opts_tmpfiles_d_lines,
};
