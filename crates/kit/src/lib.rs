//! bcvk library - exposes internal modules for testing

pub mod cpio;
pub mod qemu_img;
pub mod ssh_options;
pub mod xml_utils;

// Cross-platform modules
pub mod ssh_options;

// Linux-only modules
#[cfg(target_os = "linux")]
pub mod kernel;

// macOS-only modules (vfkit backend)
#[cfg(target_os = "macos")]
pub mod nbdkit_macos;
#[cfg(target_os = "macos")]
pub mod run_ephemeral_macos;

#[cfg(target_os = "macos")]
pub mod vfkit;
