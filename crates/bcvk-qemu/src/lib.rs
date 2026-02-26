//! QEMU virtualization library with virtiofs support.
//!
//! This crate provides a Rust interface for launching and managing QEMU virtual
//! machines with VirtIO devices, particularly virtiofs filesystem mounts.
//!
//! # Features
//!
//! - **QEMU VM Management**: Launch VMs with direct kernel boot, virtio devices,
//!   and automatic resource cleanup
//! - **VirtioFS Mounts**: Spawn and manage virtiofsd processes for sharing host
//!   directories with the guest
//! - **SMBIOS Credentials**: Inject systemd credentials via QEMU SMBIOS interface
//!   for passwordless authentication and configuration
//! - **VirtIO Serial**: Configure virtio-serial devices for guest-to-host
//!   communication (e.g., log streaming)
//!
//! # Example
//!
//! ```no_run
//! use bcvk_qemu::{QemuConfig, VirtiofsConfig};
//!
//! # async fn example() -> color_eyre::Result<()> {
//! // Configure a VM with direct kernel boot
//! let mut config = QemuConfig::new_direct_boot(
//!     2048,  // memory_mb
//!     2,     // vcpus
//!     "/path/to/kernel".to_string(),
//!     "/path/to/initramfs".to_string(),
//!     "/tmp/virtiofs.sock".into(),
//! );
//!
//! // Add kernel command line arguments
//! config.set_kernel_cmdline(vec![
//!     "console=ttyS0".to_string(),
//!     "root=rootfs".to_string(),
//!     "rw".to_string(),
//! ]);
//!
//! // Enable console output
//! config.set_console(true);
//! # Ok(())
//! # }
//! ```

mod credentials;
mod qemu;
mod virtiofsd;

pub use credentials::{
    generate_virtiofs_mount_unit, guest_path_to_unit_name, key_to_root_tmpfiles_d,
    smbios_cred_for_root_ssh, smbios_cred_for_vsock_notify, smbios_creds_for_mount_unit,
    smbios_creds_for_storage_opts, storage_opts_tmpfiles_d_lines,
};

pub use qemu::{
    BootMode, DiskFormat, DisplayMode, NetworkMode, QemuConfig, ResourceLimits, RunningQemu,
    VirtioBlkDevice, VirtioSerialOut, VirtiofsMount, VHOST_VSOCK,
};

pub use virtiofsd::{spawn_virtiofsd_async, validate_virtiofsd_config, VirtiofsConfig};
