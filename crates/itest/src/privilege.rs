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
//!
//! Before launching a VM, `require_root` acquires tokens from the
//! global VM jobserver (one token per 128 MiB of VM memory).  This
//! limits total concurrent VM memory to what the host can sustain.

use crate::resources::{
    itype_memory_mib, vm_jobserver, DEFAULT_VM_MEMORY_MIB, DEFAULT_VM_VCPUS, TOKEN_MIB,
};
use crate::{TestError, VmOptions};
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
/// Before launching a VM, acquires tokens from the global VM
/// jobserver — one token per 128 MiB of VM memory.  This ensures
/// concurrent VMs don't exceed the host's memory budget, regardless
/// of the test runner being used.
///
/// # Arguments
///
/// * `test_name` — the name passed to `--exact` when re-invoking.
/// * `test_binary` — binary name or path invoked inside the VM.
/// * `mode` — [`DispatchMode::Privileged`] or [`DispatchMode::Booted`].
/// * `vm_options` — VM sizing options (instance type, etc.).
///
/// # Environment variables
///
/// * `BCVK_PATH` — path to the bcvk binary (default: `"bcvk"`).
/// * `ITEST_IMAGE` — container image to boot in the VM (**required**
///   when not root).
/// * `ITEST_IN_VM` — recursion guard: if set we expect to already be
///   root; if not, something is broken.
pub fn require_root(
    test_name: &str,
    test_binary: &str,
    mode: DispatchMode,
    vm_options: &VmOptions,
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

    // Determine VM memory for jobserver token count.
    // Priority: itype → look up its memory; explicit memory_mib; default.
    let memory_mib = match vm_options.itype {
        Some(it) => itype_memory_mib(it).unwrap_or(DEFAULT_VM_MEMORY_MIB),
        None => vm_options.memory_mib.unwrap_or(DEFAULT_VM_MEMORY_MIB),
    };

    // Acquire jobserver tokens (1 token = 128 MiB, rounded up)
    let tokens = (memory_mib + TOKEN_MIB - 1) / TOKEN_MIB;
    let _permit = vm_jobserver().acquire(tokens).map_err(|e| -> TestError {
        format!("failed to acquire {tokens} VM token(s): {e}").into()
    })?;

    let sh = Shell::new()?;
    let bcvk = std::env::var("BCVK_PATH").unwrap_or_else(|_| "bcvk".into());
    let in_vm_env = "ITEST_IN_VM=1";

    // Build VM sizing arguments.
    let mut vm_args: Vec<String> = Vec::new();

    if let Some(itype) = vm_options.itype {
        vm_args.push("--itype".into());
        vm_args.push(itype.into());
    } else {
        let mem = vm_options.memory_mib.unwrap_or(DEFAULT_VM_MEMORY_MIB);
        let cpus = vm_options.vcpus.unwrap_or(DEFAULT_VM_VCPUS);

        vm_args.push("--memory".into());
        vm_args.push(format!("{mem}M"));
        vm_args.push("--vcpus".into());
        vm_args.push(cpus.to_string());
    }

    match mode {
        DispatchMode::Booted => {
            let vm_name = format!("itest-{}", test_name.replace('_', "-"));
            cmd!(
                sh,
                "{bcvk} libvirt run --name {vm_name} --replace --detach --ssh-wait {vm_args...} {image}"
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
                "{bcvk} ephemeral run-ssh {vm_args...} {image} -- env {in_vm_env} {test_binary} --exact {test_name}"
            )
            .run()?;
        }
    }

    // _permit dropped here → tokens returned to pipe
    Ok(Some(()))
}
