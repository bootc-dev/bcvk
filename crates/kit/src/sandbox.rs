//! Container-side setup for the VM supervisor.
//!
//! bcvk runs QEMU and virtiofsd from the host's `/usr`, which podman bind-mounts
//! into the container at `/run/tmproot/usr`. Those processes need that hybrid
//! tree as their root, so the supervisor assembles it, makes it this process's
//! root with pivot_root(2), and only then execs anything out of it.

use std::path::Path;
use std::process::Command;

use color_eyre::eyre::{self, eyre, Context as _};
use color_eyre::Result;
use rustix::mount::{
    mount, mount_bind_recursive, mount_change, unmount, MountFlags, MountPropagationFlags,
    UnmountFlags,
};
use rustix::process::pivot_root;
use rustix::thread::{unshare_unsafe, UnshareFlags};
use tracing::debug;

pub const TMPROOT: &str = "/run/tmproot";

/// Holds the virtiofsd sockets, shared with processes outside this namespace.
const SOCKETS: &str = "/run/inner-shared";

/// The target image's systemd version, read before the root change puts the
/// host's `/usr` in place of the image's.
static SYSTEMD_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Assemble the hybrid root and make it this process's root.
///
/// Only the VM supervisor calls this. Anything else arrives later through
/// `podman exec`, which joins the supervisor's namespaces and so is already in
/// the hybrid root.
pub fn setup() -> Result<()> {
    init_tmproot().context("Assembling hybrid root")?;
    let _ = SYSTEMD_VERSION.set(read_systemd_version());
    enter(TMPROOT)
}

/// The target image's systemd version output, if it reported one.
pub fn systemd_version() -> Option<&'static str> {
    SYSTEMD_VERSION
        .get()
        .map(String::as_str)
        .filter(|v| !v.is_empty())
}

fn init_tmproot() -> Result<()> {
    let root = Path::new(TMPROOT);

    for (target, source) in [
        ("bin", "usr/bin"),
        ("lib", "usr/lib"),
        ("lib64", "usr/lib64"),
        ("sbin", "usr/sbin"),
    ] {
        let target = root.join(target);
        std::os::unix::fs::symlink(source, &target)
            .with_context(|| format!("Creating {target:?}"))?;
    }
    for dir in ["etc", "var", "var/tmp", "dev", "proc", "run", "sys", "tmp"] {
        std::fs::create_dir_all(root.join(dir))?;
    }

    // ssh-keygen wants /etc/passwd to exist.
    let st = Command::new("systemd-sysusers")
        .arg("--root")
        .arg(root)
        .output()
        .context("Running systemd-sysusers")?;
    eyre::ensure!(
        st.status.success(),
        "systemd-sysusers failed: {}",
        String::from_utf8_lossy(&st.stderr).trim()
    );

    // QEMU's user-mode networking resolves DNS with the resolv.conf podman
    // wrote for the container, which is outside the new root.
    if Path::new("/etc/resolv.conf").exists() {
        std::fs::copy("/etc/resolv.conf", root.join("etc/resolv.conf"))?;
    }

    std::fs::create_dir(SOCKETS)?;
    Ok(())
}

/// Ask the image's systemctl for its version. An image that cannot report one
/// yields an empty string, which callers treat as unknown.
fn read_systemd_version() -> String {
    Command::new("systemctl")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn enter(newroot: &str) -> Result<()> {
    let root = Path::new(newroot);
    if !root.join("usr").exists() {
        return Err(eyre!(
            "{newroot}/usr does not exist: the container was not set up by bcvk"
        ));
    }

    // A new mount namespace applies to the calling thread only, so this has to
    // run before the tokio runtime exists.
    //
    // SAFETY: unshare is unsafe only for UnshareFlags::FILES, where one thread
    // can be left unable to use another's file descriptors.
    #[allow(unsafe_code)]
    unsafe { unshare_unsafe(UnshareFlags::NEWNS) }.context("Unsharing mount namespace")?;

    // Keep these mounts out of the container's mount namespace, which is the
    // one `podman exec` joins.
    mount_change(
        "/",
        MountPropagationFlags::REC | MountPropagationFlags::DOWNSTREAM,
    )
    .context("Making / slave")?;

    // pivot_root(2) requires the new root to be a mount point of its own. The
    // bind has to be recursive, or podman's mount of the host /usr at
    // <newroot>/usr is left behind and the new root has no binaries in it.
    mount_bind_recursive(newroot, newroot).context("Binding new root onto itself")?;

    let proc = root.join("proc");
    mount("proc", &proc, "proc", MountFlags::empty(), None).context("Mounting /proc")?;

    // /run is shared rather than private: the virtiofsd sockets, the status
    // file the monitor watches, and the mounted source image all live there and
    // are reached from outside this namespace.
    for (source, target) in [("/dev", "dev"), ("/var/tmp", "var/tmp"), ("/run", "run")] {
        mount_bind_recursive(source, root.join(target))
            .with_context(|| format!("Binding {source}"))?;
    }

    // Passing "." for both arguments avoids needing a put_old directory.
    std::env::set_current_dir(newroot)?;
    pivot_root(".", ".").context("pivot_root")?;
    unmount(".", UnmountFlags::DETACH).context("Detaching old root")?;
    std::env::set_current_dir("/")?;

    debug!("Root is now {newroot}");
    Ok(())
}
