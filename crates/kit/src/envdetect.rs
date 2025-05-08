//! Output information about the environment

use std::{os::unix::fs::MetadataExt, path::Path};

use anyhow::{Context, Result};

use cap_std_ext::cap_std;
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

impl Environment {
    pub fn new() -> Result<Self> {
        let rootfs = super::containerenv::global_rootfs(cap_std::ambient_authority())?;
        let privileged =
            rustix::thread::capability_is_in_bounding_set(rustix::thread::Capability::SystemAdmin)?;
        let container = super::containerenv::is_container(&rootfs)?;
        let containerenv =
            super::containerenv::get_cached_container_execution_info(&rootfs)?.cloned();
        let pidhost = is_hostpid()?;
        Ok(Environment {
            privileged,
            pidhost,
            containerenv,
            container,
        })
    }
}
