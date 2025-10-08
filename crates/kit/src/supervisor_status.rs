use cap_std_ext::{cap_std, cap_std::fs::Dir, dirext::CapStdExtDirExt};
use color_eyre::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Status of the supervisor process and VM
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SupervisorStatus {
    /// Current state of the supervisor/VM
    pub state: Option<SupervisorState>,
    /// If we saw ssh-access.target
    pub ssh_access: bool,
    /// True if qemu is running
    pub running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorState {
    /// Waiting for systemd to become ready
    WaitingForSystemd,
    /// Systemd reached a specific target
    ReachedTarget(String),
    /// VM is ready and accepting connections
    Ready,
}

impl SupervisorStatus {
    /// Create a new status with the given state
    pub fn new(state: SupervisorState) -> Self {
        Self {
            state: Some(state),
            running: true,
            ..Default::default()
        }
    }

    /// Write status to a JSON file atomically
    pub fn write_to_file(&self, path: impl AsRef<Path>) -> color_eyre::Result<()> {
        let path = path.as_ref();
        let json = serde_json::to_string_pretty(self)?;

        // Get parent directory for atomic write
        let parent = path.parent().unwrap_or(Path::new("/"));
        let filename = path.file_name().unwrap_or_else(|| path.as_os_str());

        let dir = Dir::open_ambient_dir(parent, cap_std::ambient_authority())?;
        dir.atomic_write(filename, json)?;

        Ok(())
    }

    /// Read status from a JSON file
    pub fn read_from_file(path: impl AsRef<Path>) -> color_eyre::Result<Self> {
        let contents = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&contents)?)
    }
}

/// Helper to write status updates from the supervisor
pub struct StatusWriter {
    path: String,
}

impl StatusWriter {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }

    pub fn update(&self, status: SupervisorStatus) -> color_eyre::Result<()> {
        status.write_to_file(&self.path)
    }

    pub fn update_state(&self, state: SupervisorState) -> color_eyre::Result<()> {
        self.update(SupervisorStatus::new(state))
    }

    pub fn finish(self) -> Result<()> {
        self.update(SupervisorStatus {
            running: false,
            ..Default::default()
        })
    }
}
