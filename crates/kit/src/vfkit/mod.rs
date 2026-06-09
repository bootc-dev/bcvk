//! Persistent VM management for macOS using vfkit + EFI boot.
//!
//! Subcommands mirror the Linux libvirt/ module structure:
//! run, list, ssh, stop, start, rm, rm-all, inspect

use std::fs;
use std::path::PathBuf;

use clap::Subcommand;
use color_eyre::Result;

pub mod inspect;
pub mod list;
pub mod rm;
pub mod rm_all;
pub mod run;
pub mod ssh;
pub mod start;
pub mod stop;

/// Output format for inspect and list commands.
#[derive(Debug, Clone, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum OutputFormat {
    /// Table format (default for list)
    Table,
    /// JSON format
    Json,
    /// YAML-like key-value format (default for inspect)
    Yaml,
    /// XML format (not yet implemented)
    Xml,
}

/// Subcommands for persistent VM management via vfkit.
#[derive(Debug, Subcommand)]
pub enum VmCommands {
    /// Run a persistent VM from a disk image
    Run(run::VmRunOpts),

    /// List all persistent VMs
    #[clap(name = "list", alias = "ls")]
    List(list::VmListOpts),

    /// SSH into a running VM
    Ssh(ssh::VmSshOpts),

    /// Stop a running VM
    Stop(stop::VmStopOpts),

    /// Start a stopped VM
    Start(start::VmStartOpts),

    /// Remove a VM and its metadata
    #[clap(name = "rm")]
    Remove(rm::VmRmOpts),

    /// Remove all VMs
    #[clap(name = "rm-all")]
    RemoveAll(rm_all::VmRmAllOpts),

    /// Show detailed VM information
    Inspect(inspect::VmInspectOpts),
}

impl VmCommands {
    /// Dispatch to the appropriate subcommand handler.
    pub fn run(self) -> Result<()> {
        match self {
            VmCommands::Run(opts) => run::run(opts),
            VmCommands::List(opts) => list::run(opts),
            VmCommands::Ssh(opts) => ssh::run(opts),
            VmCommands::Stop(opts) => stop::run(opts),
            VmCommands::Start(opts) => start::run(opts),
            VmCommands::Remove(opts) => rm::run(opts),
            VmCommands::RemoveAll(opts) => rm_all::run(opts),
            VmCommands::Inspect(opts) => inspect::run(opts),
        }
    }
}

// --- VM Metadata ---

/// Persistent VM metadata, stored as JSON in `~/.local/share/bcvk/vms/`.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct VmMetadata {
    /// VM name used as identifier.
    pub name: String,
    /// Container image used to create this VM (None if created from disk directly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Path to the disk image file.
    pub disk_image: String,
    /// PID of the vfkit process.
    pub vfkit_pid: u32,
    /// PID of the gvproxy network proxy process.
    pub gvproxy_pid: u32,
    /// Host-side SSH port forwarded to the VM.
    pub ssh_port: u16,
    /// Path to the SSH private key.
    pub ssh_key: String,
    /// Number of vCPUs allocated.
    pub vcpus: u32,
    /// Memory in megabytes.
    pub memory_mb: u32,
    /// Path to the EFI variable store file.
    pub efi_store: String,
    /// Path to the serial console log file.
    pub serial_log: String,
    /// Whether GUI mode is enabled.
    pub gui: bool,
    /// ISO 8601 timestamp when the VM was created.
    pub created: String,
    /// Current VM state (running, stopped).
    pub state: String,
    /// User-defined labels for organizing VMs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Port mappings from host to VM (host_port, guest_port).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub port_mappings: Vec<(u16, u16)>,
}

impl VmMetadata {
    /// Return the directory path for persistent VM metadata files.
    pub fn vms_dir() -> PathBuf {
        dirs::home_dir()
            .expect("cannot determine home directory")
            .join(".local/share/bcvk/vms")
    }

    /// Save metadata to a JSON file in the VMs directory.
    pub fn save(&self) -> Result<()> {
        let dir = Self::vms_dir();
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", self.name));
        fs::write(&path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Load metadata for the named VM from its JSON file.
    pub fn load(name: &str) -> Result<Self> {
        let path = Self::vms_dir().join(format!("{}.json", name));
        let data = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&data)?)
    }

    /// Remove metadata file for the named VM.
    pub fn remove(name: &str) {
        let path = Self::vms_dir().join(format!("{}.json", name));
        crate::vm_helpers::remove_file_if_exists(&path);
    }

    /// List all persistent VM metadata from the VMs directory.
    pub fn list_all() -> Result<Vec<Self>> {
        let dir = Self::vms_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut vms = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(meta) = serde_json::from_str::<Self>(&data) {
                    vms.push(meta);
                }
            }
        }
        Ok(vms)
    }

    /// Check if the VM process is still alive via kill(pid, 0).
    pub fn is_alive(&self) -> bool {
        if self.vfkit_pid == 0 {
            return false;
        }
        rustix::process::test_kill_process(
            rustix::process::Pid::from_raw(self.vfkit_pid as i32).unwrap(),
        )
        .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_vm_metadata(name: &str) -> VmMetadata {
        VmMetadata {
            name: name.to_string(),
            image: None,
            disk_image: "/tmp/disk.raw".to_string(),
            vfkit_pid: 0,
            gvproxy_pid: 0,
            ssh_port: 2222,
            ssh_key: "/tmp/key".to_string(),
            vcpus: 2,
            memory_mb: 4096,
            efi_store: "/tmp/efi.fd".to_string(),
            serial_log: "/tmp/serial.log".to_string(),
            gui: false,
            created: "2026-01-01T00:00:00Z".to_string(),
            state: "running".to_string(),
            labels: vec![],
            port_mappings: vec![],
        }
    }

    #[test]
    fn test_vm_metadata_roundtrip() {
        let meta = sample_vm_metadata("test-vm");
        let json = serde_json::to_string_pretty(&meta).unwrap();
        let loaded: VmMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.name, "test-vm");
        assert_eq!(loaded.disk_image, "/tmp/disk.raw");
        assert_eq!(loaded.vcpus, 2);
        assert_eq!(loaded.memory_mb, 4096);
        assert_eq!(loaded.state, "running");
        assert!(!loaded.gui);
    }

    #[test]
    fn test_vm_metadata_save_load_remove() {
        let dir = tempfile::tempdir().unwrap();
        let json_path = dir.path().join("myvm.json");
        let meta = sample_vm_metadata("myvm");
        fs::write(&json_path, serde_json::to_string_pretty(&meta).unwrap()).unwrap();
        let data = fs::read_to_string(&json_path).unwrap();
        let loaded: VmMetadata = serde_json::from_str(&data).unwrap();
        assert_eq!(loaded.name, "myvm");
        assert_eq!(loaded.ssh_port, 2222);
        fs::remove_file(&json_path).unwrap();
        assert!(!json_path.exists());
    }

    #[test]
    fn test_vm_metadata_list_from_dir() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..3 {
            let meta = sample_vm_metadata(&format!("vm-{i}"));
            let path = dir.path().join(format!("vm-{i}.json"));
            fs::write(&path, serde_json::to_string(&meta).unwrap()).unwrap();
        }
        fs::write(dir.path().join("notes.txt"), "ignored").unwrap();

        let mut vms = Vec::new();
        for entry in fs::read_dir(dir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(meta) = serde_json::from_str::<VmMetadata>(&data) {
                    vms.push(meta);
                }
            }
        }
        assert_eq!(vms.len(), 3);
        let mut names: Vec<_> = vms.iter().map(|v| v.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["vm-0", "vm-1", "vm-2"]);
    }

    #[test]
    fn test_vm_metadata_is_alive_zero_pid() {
        let meta = sample_vm_metadata("dead-vm");
        assert!(!meta.is_alive());
    }
}
