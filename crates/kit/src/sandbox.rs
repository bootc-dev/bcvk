//! Namespace setup for the container entrypoint.
//!
//! bcvk runs QEMU and virtiofsd from the host's `/usr`, which podman bind-mounts
//! into the container at `/run/tmproot/usr`. Those processes need that hybrid
//! tree as their root, so the entrypoint makes it this process's root with
//! pivot_root(2) before running anything out of it.

use std::path::Path;

use color_eyre::eyre::{eyre, Context as _};
use color_eyre::Result;
use rustix::mount::{
    mount, mount_bind_recursive, mount_change, unmount, MountFlags, MountPropagationFlags,
    UnmountFlags,
};
use rustix::process::pivot_root;
use rustix::thread::{unshare_unsafe, UnshareFlags};
use tracing::debug;

pub const TMPROOT: &str = "/run/tmproot";

pub fn enter(newroot: &str) -> Result<()> {
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
