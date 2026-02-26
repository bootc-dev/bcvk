//! QEMU virtualization integration and VM management.
//!
//! This module re-exports types from the bcvk-qemu crate and provides
//! integration with the kit crate's Format type.
//!
//! Supports direct kernel boot with VirtIO devices, automatic process cleanup,
//! and SMBIOS credential injection.

// Re-export all public items from bcvk-qemu
pub use bcvk_qemu::*;

use crate::to_disk::Format;
use bcvk_qemu::DiskFormat;

impl From<Format> for DiskFormat {
    fn from(format: Format) -> Self {
        match format {
            Format::Raw => DiskFormat::Raw,
            Format::Qcow2 => DiskFormat::Qcow2,
        }
    }
}

impl From<&Format> for DiskFormat {
    fn from(format: &Format) -> Self {
        match format {
            Format::Raw => DiskFormat::Raw,
            Format::Qcow2 => DiskFormat::Qcow2,
        }
    }
}

/// Add a virtio-blk device with the kit Format type.
pub trait QemuConfigExt {
    /// Add a virtio-blk device with specified format using kit's Format type.
    fn add_virtio_blk_device_with_format<F: Into<DiskFormat>>(
        &mut self,
        disk_file: String,
        serial: String,
        format: F,
    ) -> &mut Self;
}

impl QemuConfigExt for QemuConfig {
    fn add_virtio_blk_device_with_format<F: Into<DiskFormat>>(
        &mut self,
        disk_file: String,
        serial: String,
        format: F,
    ) -> &mut Self {
        self.add_virtio_blk_device(disk_file, serial, format.into())
    }
}
