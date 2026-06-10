//! bcvk library - exposes internal modules for testing

pub mod cpio;
pub mod qemu_img;
pub mod ssh_options;
pub mod xml_utils;

// Cross-platform modules
pub mod install_options;
pub mod kernel_cmdline;
pub mod ssh_options;

// Linux-only modules
#[cfg(target_os = "linux")]
pub mod kernel;
