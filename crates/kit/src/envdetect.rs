//! Output information about the environment

use anyhow::Result;

use cap_std_ext::cap_std;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Environment {
    pub privileged: bool,
    pub container: bool,
    pub containerenv: Option<super::containerenv::ContainerExecutionInfo>,
}

impl Environment {
    pub fn new() -> Result<Self> {
        let rootfs = super::containerenv::global_rootfs(cap_std::ambient_authority())?;
        let privileged = rustix::thread::capability_is_in_bounding_set(rustix::thread::Capability::SystemAdmin)?;
        let container = super::containerenv::is_container(&rootfs)?;
        let containerenv = super::containerenv::get_cached_container_execution_info(&rootfs)?.cloned();
        Ok(Environment { privileged, containerenv, container })
    }
}