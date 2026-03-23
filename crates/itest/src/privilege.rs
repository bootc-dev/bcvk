//! Privilege detection and VM dispatch.
//!
//! When a test needs root but the process is unprivileged, we
//! re-invoke the test binary inside a bcvk VM.  Two modes are
//! supported:
//!
//! * **Privileged** — `bcvk ephemeral run-ssh` (fast, no disk
//!   install).
//! * **Booted** — `bcvk libvirt run` + SSH (full disk install via
//!   `bootc install to-disk`).

use crate::TestError;
use xshell::{cmd, Shell};

/// How a test should be dispatched when not running as root.
#[derive(Debug, Clone, Copy)]
pub enum DispatchMode {
    /// Just needs root — use `bcvk ephemeral run-ssh` (no disk install).
    Privileged,
    /// Needs a fully deployed system — use `bcvk libvirt run`.
    Booted,
}

/// Check whether we are running as root and, if not, dispatch the
/// test to a bcvk VM.
///
/// * Returns `Ok(None)` when already root — the caller should run
///   the test body.
/// * Returns `Ok(Some(()))` after successfully dispatching — the
///   caller should return early.
///
/// # Arguments
///
/// * `test_name` — the name passed to `--exact` when re-invoking.
/// * `test_binary` — binary name or path invoked inside the VM.
/// * `mode` — [`DispatchMode::Privileged`] or [`DispatchMode::Booted`].
///
/// # Environment variables
///
/// * `BCVK_PATH` — path to the bcvk binary (default: `"bcvk"`).
/// * `ITEST_IMAGE` — container image to boot in the VM (**required**
///   when not root).
/// * `ITEST_IN_VM` — recursion guard: if set we expect to already be
///   root; if not, something is broken.
///
/// Projects that need different env var names should set `ITEST_IMAGE`
/// from their own project-specific variable in `main()`, or define
/// thin wrapper functions.
pub fn require_root(
    test_name: &str,
    test_binary: &str,
    mode: DispatchMode,
) -> Result<Option<()>, TestError> {
    if rustix::process::getuid().is_root() {
        return Ok(None);
    }

    // Recursion guard
    if std::env::var_os("ITEST_IN_VM").is_some() {
        return Err("ITEST_IN_VM is set but we are not root — VM setup is broken".into());
    }

    let image = std::env::var("ITEST_IMAGE").map_err(|_| -> TestError {
        "not root and ITEST_IMAGE not set; \
         set it to a bootc container image to run privileged tests"
            .into()
    })?;

    let sh = Shell::new()?;
    let bcvk = std::env::var("BCVK_PATH").unwrap_or_else(|_| "bcvk".into());

    // Pass the recursion guard so the binary knows it's inside a VM
    let in_vm_env = "ITEST_IN_VM=1";

    match mode {
        DispatchMode::Booted => {
            let vm_name = format!("itest-{}", test_name.replace('_', "-"));
            cmd!(
                sh,
                "{bcvk} libvirt run --name {vm_name} --replace --detach --ssh-wait {image}"
            )
            .run()?;

            let result = cmd!(
                sh,
                "{bcvk} libvirt ssh {vm_name} -- env {in_vm_env} {test_binary} --exact {test_name}"
            )
            .run();

            // Always clean up
            if let Err(e) = cmd!(sh, "{bcvk} libvirt rm --stop --force {vm_name}").run() {
                eprintln!("warning: failed to clean up VM {vm_name}: {e}");
            }
            result?;
        }
        DispatchMode::Privileged => {
            cmd!(
                sh,
                "{bcvk} ephemeral run-ssh {image} -- env {in_vm_env} {test_binary} --exact {test_name}"
            )
            .run()?;
        }
    }

    Ok(Some(()))
}
