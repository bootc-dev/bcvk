//! Cross-platform kernel command-line parameters shared across backends.

/// Base kernel command-line parameters common to all backends.
#[allow(dead_code)]
pub const BASE_KERNEL_CMDLINE: &[&str] = &[
    "console=hvc0",
    "selinux=0",
    "systemd.journald.storage=volatile",
];
