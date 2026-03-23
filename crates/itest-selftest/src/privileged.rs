//! Privileged self-tests for the itest framework.
//!
//! These tests use `itest::privileged_test!` — the exact same macro
//! that consumers like ostree and bootc use.  When run without root
//! they auto-dispatch to a bcvk ephemeral VM (which must have the
//! binary installed — see the Containerfile).
//!
//! To run:
//!   cd crates/itest-selftest && just

/// Binary name as installed inside the container image.
const BIN: &str = "itest-selftest";

itest::privileged_test!(BIN, selftest_is_root, {
    if !rustix::process::getuid().is_root() {
        let e: itest::TestError = "expected to be running as root (uid 0)".into();
        return Err(e);
    }
    Ok(())
});
