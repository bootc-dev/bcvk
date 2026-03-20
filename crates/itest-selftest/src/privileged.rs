//! Privileged self-tests for the itest framework.
//!
//! These tests use `itest::privileged_test!` — the exact same macro
//! that consumers like ostree and bootc use.  When run without root
//! they auto-dispatch to a bcvk ephemeral VM (which must have the
//! binary installed — see the Containerfile).
//!
//! To run:
//!   # Build the test container image first:
//!   podman build -t localhost/itest-selftest:latest \
//!     -f crates/itest-selftest/Containerfile .
//!
//!   # Then run:
//!   ITEST_IMAGE=localhost/itest-selftest:latest \
//!   BCVK_PATH=target/debug/bcvk \
//!     cargo test -p itest-selftest --test itest-selftest

use anyhow::ensure;

/// Binary name as installed inside the container image.
const BIN: &str = "itest-selftest";

// ── privileged_test! tests ──────────────────────────────────────────
//
// Each of these will:
//   - If root: run the body directly.
//   - If not root: dispatch to bcvk ephemeral run-ssh and re-invoke
//     `itest-selftest --exact <test_name>` inside the VM.

itest::privileged_test!(BIN, selftest_is_root, {
    ensure!(
        rustix::process::getuid().is_root(),
        "expected to be running as root (uid 0)"
    );
    Ok(())
});

itest::privileged_test!(BIN, selftest_has_kernel, {
    ensure!(
        rustix::process::getuid().is_root(),
        "expected to be running as root"
    );
    ensure!(
        std::path::Path::new("/proc/1/status").exists(),
        "/proc/1/status should exist in a booted VM"
    );
    let uname = rustix::system::uname();
    let release = uname.release().to_string_lossy();
    ensure!(!release.is_empty(), "kernel release should not be empty");
    Ok(())
});

itest::privileged_test!(BIN, selftest_root_caps, {
    ensure!(
        rustix::process::getuid().is_root(),
        "expected to be running as root"
    );
    // chown requires CAP_CHOWN — would fail in a user namespace
    // without real root.
    let tmp = tempfile::tempdir_in("/tmp")?;
    let path = tmp.path().join("root-test");
    std::fs::write(&path, "hello from root")?;
    rustix::fs::chown(&path, Some(rustix::process::Uid::ROOT), None)?;
    Ok(())
});
