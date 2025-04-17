//! Implementation of the `run-rmvm` command for running bootc containers in ephemeral VMs
//!
//! This creates an ephemeral VM instantiated from a bootc container image
//! and logs in over SSH.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;

use bootc_utils::CommandRunExt;
use clap::{self, Parser};
use color_eyre::{eyre::eyre, Result};
use rand::distr::SampleString;
use tempfile::TempDir;
use tracing::instrument;

use crate::{hostexec, images, virtinstall::FromSRBOpts, virtinstall::VirtInstallOpts};

/// Options for the run-rmvm command
#[derive(Parser, Debug)]
pub struct RunRmVmOpts {
    /// Name of the image to run
    pub image: String,

    /// Path to SSH key to use (defaults to ~/.ssh/id_rsa.pub)
    #[clap(long)]
    pub sshkey: Option<String>,

    /// Memory to allocate to the VM in MB
    #[clap(long, default_value = "4096")]
    pub memory: u32,

    /// Number of vCPUs to allocate
    #[clap(long, default_value_t = 2)]
    pub vcpus: u32,

    /// Size of the VM disk in GB
    #[clap(long, default_value_t = 10)]
    pub size: u32,
}

impl RunRmVmOpts {
    #[instrument]
    pub(crate) fn run(&self) -> Result<()> {
        println!("Creating ephemeral VM from {}", self.image);

        // Verify the image exists
        let _inspect = images::inspect(&self.image)?;

        // Determine the SSH key to use
        let sshkey_path = if let Some(key) = &self.sshkey {
            key.clone()
        } else {
            let home = std::env::var("HOME").map_err(|e| eyre!("Querying $HOME: {}", e))?;
            format!("{}/.ssh/id_rsa.pub", home)
        };

        // Check if the SSH key exists
        if !Path::new(&sshkey_path).exists() {
            return Err(eyre!("SSH key not found: {}", sshkey_path));
        }

        // Verify we can read the SSH key
        std::fs::read_to_string(&sshkey_path)
            .map_err(|e| eyre!("Reading SSH key from {}: {}", sshkey_path, e))?;

        // Create a temporary directory for VM-related files
        let temp_dir = TempDir::new().map_err(|e| eyre!("Creating temporary directory: {}", e))?;
        let temp_path = temp_dir.path();

        // Create a name for the VM
        let random_suffix = rand::distr::Alphanumeric.sample_string(&mut rand::rng(), 8);
        let vm_name = format!("bootc-ephemeral-{}", random_suffix);

        // Set up virt-install options
        let virt_install_opts = VirtInstallOpts::FromSRB(FromSRBOpts {
            libvirt_opts: Default::default(),
            image: self.image.clone(),
            transient: true, // Always use transient for ephemeral VMs
            skip_bind_storage: false,
            base_volume: None,
            name: Some(vm_name.clone()),
            sshkey: Some(sshkey_path.clone()),
            size: self.size,
            vcpus: self.vcpus,
            memory: self.memory,
            vinstarg: vec![],
        });

        // Run virt-install to create the VM
        virt_install_opts.run()?;

        // Wait for the VM to be ready and connect via SSH
        println!("VM is being created. Waiting for it to be ready...");

        // Write an SSH config file for this VM
        let ssh_config_path = temp_path.join("ssh_config");
        let mut ssh_config =
            File::create(&ssh_config_path).map_err(|e| eyre!("Creating SSH config file: {}", e))?;

        write!(
            ssh_config,
            r#"Host {}
  User root
  StrictHostKeyChecking no
  UserKnownHostsFile /dev/null
  IdentityFile {}
"#,
            vm_name,
            sshkey_path.replace(".pub", "")
        )
        .map_err(|e| eyre!("Writing SSH config: {}", e))?;

        // Try to connect via SSH (with retries)
        let max_retries = 60;
        let mut successful = false;

        for i in 1..=max_retries {
            println!(
                "Attempting to connect to VM (attempt {}/{})",
                i, max_retries
            );

            let status = hostexec::command("ssh", None)?
                .args([
                    "-F",
                    ssh_config_path.to_str().unwrap(),
                    &vm_name,
                    "echo 'Connected successfully'",
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|e| eyre!("Running SSH test: {}", e))?;

            if status.success() {
                successful = true;
                break;
            }

            std::thread::sleep(std::time::Duration::from_secs(1));
        }

        if !successful {
            return Err(eyre!(
                "Failed to connect to VM after {} attempts",
                max_retries
            ));
        }

        println!("Connected to VM. Starting SSH session...");

        // Connect via SSH
        let ssh_result = hostexec::command("ssh", None)?
            .args(["-F", ssh_config_path.to_str().unwrap(), &vm_name])
            .run();

        // Clean up the VM after the SSH session ends
        println!("SSH session ended. Cleaning up VM...");

        hostexec::command("virsh", None)?
            .args(["destroy", &vm_name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| eyre!("Destroying VM: {}", e))?;

        println!("VM has been destroyed.");

        // Return the SSH session result
        match ssh_result {
            Ok(_) => Ok(()),
            Err(e) => Err(eyre!("SSH session error: {}", e)),
        }
    }
}
