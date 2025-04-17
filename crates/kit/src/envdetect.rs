//! Output information about the environment

use std::{
    os::unix::fs::MetadataExt,
    path::Path,
    sync::{Arc, OnceLock},
};

use color_eyre::{eyre::Context, Result};

use cap_std_ext::cap_std::{self, fs::Dir};
use serde::{Deserialize, Serialize};

/// Data we've discovered about the ambient environment
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Environment {
    /// Run with --privileged
    pub privileged: bool,
    /// Run with --pid=host
    pub pidhost: bool,
    /// Detected /run/.containerenv (which is present but empty without --privileged)
    pub container: bool,
    /// The full parsed contents of /run/.containerenv
    pub containerenv: Option<super::containerenv::ContainerExecutionInfo>,
}

/// Check if this process is running with --pid=host
fn is_hostpid() -> Result<bool> {
    let Some(ppid) = rustix::process::getppid() else {
        return Ok(false);
    };
    let myuid = rustix::process::getuid();
    let parent_proc = format!("/proc/{}", ppid.as_raw_nonzero());
    let parent_st = Path::new(&parent_proc).metadata()?;
    // If the parent has a different uid, that's a strong signal we're
    // running with a uid mapping but we can see our real parent in the
    // host pidns.
    if parent_st.uid() != myuid.as_raw() {
        return Ok(true);
    }
    let parent_rootns = std::fs::read_link(format!("/proc/{}/ns/mnt", ppid.as_raw_nonzero()))
        .context("Reading parent mountns")?;
    let my_rootns = std::fs::read_link("/proc/self/ns/mnt").context("Reading self mountns")?;
    Ok(parent_rootns != my_rootns)
}

/// Return a cached copy of the global rootfs
pub(crate) fn global_rootfs(authority: cap_std::AmbientAuthority) -> Result<Arc<Dir>> {
    static ROOTFS: OnceLock<Arc<Dir>> = OnceLock::new();
    if let Some(r) = ROOTFS.get() {
        return Ok(r.clone());
    }
    let r = Dir::open_ambient_dir("/", authority)?;
    let _ = ROOTFS.set(Arc::new(r));
    Ok(ROOTFS.get().unwrap().clone())
}

impl Environment {
    fn new() -> Result<Self> {
        let rootfs = &global_rootfs(cap_std::ambient_authority())?;
        let privileged =
            rustix::thread::capability_is_in_bounding_set(rustix::thread::Capability::SystemAdmin)?;
        let container = super::containerenv::is_container(&rootfs)?;
        let containerenv = super::containerenv::get_container_execution_info(&rootfs)?;
        let pidhost = is_hostpid()?;
        Ok(Environment {
            privileged,
            pidhost,
            containerenv,
            container,
        })
    }

    pub fn get_cached() -> Result<&'static Self> {
        static INFO: std::sync::OnceLock<Environment> = std::sync::OnceLock::new();
        if let Some(r) = INFO.get() {
            return Ok(r);
        }
        let r = Self::new()?;
        // Discard duplicate init attempts
        let _ = INFO.set(r);
        // SAFETY: We know this was initialized
        Ok(INFO.get().unwrap())
    }
}
