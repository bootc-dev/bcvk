//! Common VM state management utilities

use color_eyre::Result;

use crate::domain_list::DomainLister;
use crate::libvirt::{self, LibvirtOptions};

/// VM state enumeration
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmState {
    Running,
    ShutOff,
    Paused,
    Other(String),
}

impl VmState {
    /// Parse state string from libvirt
    pub fn from_str(state: &str) -> Self {
        match state {
            "running" => VmState::Running,
            "shut off" => VmState::ShutOff,
            "paused" => VmState::Paused,
            other => VmState::Other(other.to_string()),
        }
    }
}

/// Check if a VM exists and return its state
pub fn get_vm_state(name: &str, libvirt_opts: &LibvirtOptions) -> Result<Option<VmState>> {
    let lister = if let Some(ref uri) = libvirt_opts.connect {
        DomainLister::with_connection(uri.clone())
    } else {
        DomainLister::new()
    };
    let domains = lister.list_bootc_domains()?;

    for domain in domains {
        if domain.name == name {
            return Ok(Some(VmState::from_str(&domain.state)));
        }
    }

    Ok(None)
}

/// Start a VM
pub fn start_vm(name: &str, libvirt_opts: &LibvirtOptions) -> Result<()> {
    let start_opts = libvirt::start::LibvirtStartOpts {
        name: name.to_string(),
        ssh: false,
    };

    libvirt::start::run(libvirt_opts, start_opts)
}

/// Ensure a VM is running, starting it if necessary
pub fn ensure_vm_running(name: &str, libvirt_opts: &LibvirtOptions) -> Result<()> {
    let state = get_vm_state(name, libvirt_opts)?.ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "Project VM '{}' not found. Run 'bcvk project up' first.",
            name
        )
    })?;

    match state {
        VmState::Running => {
            // Already running, nothing to do
            Ok(())
        }
        VmState::ShutOff | VmState::Paused => {
            println!("Starting project VM '{}'...", name);
            start_vm(name, libvirt_opts)
        }
        VmState::Other(state_str) => {
            color_eyre::eyre::bail!(
                "Project VM '{}' is in unexpected state '{}'. \
                 Please check the VM status manually.",
                name,
                state_str
            );
        }
    }
}
