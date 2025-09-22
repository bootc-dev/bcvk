//! Create and start libvirt domains from bootc volumes
//!
//! This module provides functionality to create libvirt domains using existing
//! bootc volumes in storage pools, with automatic resource configuration and
//! metadata-driven setup.

use crate::common_opts::{MemoryOpts, DEFAULT_MEMORY_USER_STR};
use crate::images;
use crate::install_options::InstallOptions;
use crate::libvirt::domain::DomainBuilder;
use crate::libvirt::upload::LibvirtUploadOpts;
use crate::run_ephemeral::default_vcpus;
use crate::ssh::generate_ssh_keypair;
use crate::sshcred::smbios_cred_for_root_ssh;
use crate::xml_utils;
use base64::Engine;
use camino::{Utf8Path, Utf8PathBuf};
use clap::Parser;
use color_eyre::{eyre::eyre, Result};
use std::fs;
use std::process::Command;
use tempfile;
use tracing::{debug, warn};

/// Configuration options for creating a libvirt domain from bootc volume
#[derive(Debug, Parser)]
pub struct LibvirtCreateOpts {
    /// Name of the bootc volume to use for domain creation, OR container image to create from
    /// If this looks like a container image (contains '/' or ':'), will automatically upload if needed
    pub volume_name_or_image: String,

    /// Libvirt storage pool name
    #[clap(long, default_value = "default")]
    pub pool: String,

    /// Name for the libvirt domain (defaults to volume name)
    #[clap(long)]
    pub domain_name: Option<String>,

    /// Memory size for the domain (e.g. 2G, 1024M)
    #[clap(long, default_value = DEFAULT_MEMORY_USER_STR)]
    pub memory: String,

    /// Number of vCPUs for the domain
    #[clap(long)]
    pub vcpus: Option<u32>,

    /// Network configuration (default, bridge=name, none)
    #[clap(long, default_value = "default")]
    pub network: String,

    /// Start the domain after creation
    #[clap(long)]
    pub start: bool,

    /// Enable VNC console access
    #[clap(long)]
    pub vnc: bool,

    /// Hypervisor connection URI (e.g., qemu:///system, qemu+ssh://host/system)
    #[clap(short = 'c', long = "connect")]
    pub connect: Option<String>,

    /// VNC port (default: auto-assign)
    #[clap(long)]
    pub vnc_port: Option<u16>,

    /// Additional kernel arguments
    #[clap(long)]
    pub karg: Vec<String>,

    /// Dry run - validate configuration without creating domain
    #[clap(long)]
    pub dry_run: bool,

    /// Force creation even if domain already exists
    #[clap(long)]
    pub force: bool,

    /// Installation options for automatic upload (filesystem, root-size, storage-path)
    #[clap(flatten)]
    pub install: InstallOptions,

    /// Size of the disk image for automatic upload (e.g., '20G', '10240M')
    #[clap(long)]
    pub disk_size: Option<String>,

    /// Memory size for installation VM during auto-upload (e.g. 2G, 1024M)
    #[clap(flatten)]
    pub install_memory: MemoryOpts,

    /// Number of vCPUs for installation VM during auto-upload
    #[clap(long)]
    pub install_vcpus: Option<u32>,

    /// Generate ephemeral SSH keypair and inject into domain
    #[clap(long)]
    pub generate_ssh_key: bool,

    /// Path to existing SSH private key to use (public key must exist at <key>.pub)
    #[clap(long)]
    pub ssh_key: Option<String>,

    /// SSH port for port forwarding (default: auto-assign)
    #[clap(long)]
    pub ssh_port: Option<u16>,
}

/// Metadata extracted from bootc volume
#[derive(Debug)]
pub struct BootcVolumeMetadata {
    pub source_image: Option<String>,
}

/// SSH configuration for domain
#[derive(Debug, Clone)]
pub struct SshConfig {
    pub private_key_content: String,
    pub public_key: String,
    pub port: u16,
    pub is_generated: bool,
}

impl LibvirtCreateOpts {
    /// Build a virsh command with optional connection URI
    fn virsh_command(&self) -> Command {
        let mut cmd = Command::new("virsh");
        if let Some(ref connect) = self.connect {
            cmd.arg("-c").arg(connect);
        }
        cmd
    }

    /// Check if the input appears to be a container image (vs volume name)
    fn is_container_image(&self) -> bool {
        // Container images typically contain '/' or ':' characters
        // Volume names are typically simple strings without these
        self.volume_name_or_image.contains('/')
            || (self.volume_name_or_image.contains(':')
                && !self.volume_name_or_image.ends_with(".raw"))
    }

    /// Generate domain name from volume name or container image if not specified
    fn get_domain_name(&self) -> String {
        self.domain_name.clone().unwrap_or_else(|| {
            if self.is_container_image() {
                // For container images, create a sanitized name
                let image_name = self.volume_name_or_image.clone();
                let name = image_name
                    .split('/')
                    .last()
                    .unwrap_or(&image_name)
                    .replace(':', "-")
                    .replace('/', "-")
                    .replace('.', "-");
                format!("bootc-{}", name)
            } else {
                format!("bootc-{}", self.volume_name_or_image)
            }
        })
    }

    /// Find existing volume by container image digest using name-based lookup
    fn find_cached_volume(&self, image_digest: &str) -> Result<Option<String>> {
        debug!("Looking for cached volume with digest: {}", image_digest);

        // Create a temporary upload opts to get the expected cached volume name
        let temp_upload_opts = LibvirtUploadOpts {
            source_image: self.volume_name_or_image.clone(),
            volume_name: None,
            pool: self.pool.clone(),
            disk_size: None,
            install: self.install.clone(),
            memory: MemoryOpts {
                memory: DEFAULT_MEMORY_USER_STR.to_string(),
            },
            vcpus: Some(default_vcpus()),
            karg: vec![],
            connect: self.connect.clone(),
        };

        let expected_volume_name = temp_upload_opts.get_cached_volume_name(image_digest);
        let expected_volume_path = format!("{}.raw", expected_volume_name);

        // Check if this specific volume exists
        let output = self
            .virsh_command()
            .args(&["vol-info", &expected_volume_path, "--pool", &self.pool])
            .output()?;

        if output.status.success() {
            debug!("Found cached volume: {}", expected_volume_name);
            return Ok(Some(expected_volume_name));
        }

        debug!("No cached volume found for digest: {}", image_digest);
        Ok(None)
    }

    /// Automatically upload container image if no cached volume exists
    fn ensure_volume_exists(&self) -> Result<String> {
        if !self.is_container_image() {
            // If it's already a volume name, just return it
            return Ok(self.volume_name_or_image.clone());
        }

        // It's a container image, check for cached volume
        let image_digest = images::get_image_digest(&self.volume_name_or_image)?;

        if let Some(cached_volume) = self.find_cached_volume(&image_digest)? {
            debug!("Using cached volume: {}", cached_volume);
            return Ok(cached_volume);
        }

        // No cached volume found, need to upload
        debug!(
            "No cached volume found, uploading container image: {}",
            self.volume_name_or_image
        );

        let upload_opts = LibvirtUploadOpts {
            source_image: self.volume_name_or_image.clone(),
            volume_name: None, // Let it auto-generate
            pool: self.pool.clone(),
            disk_size: self.disk_size.clone(),
            install: self.install.clone(),
            memory: self.install_memory.clone(),
            vcpus: self.install_vcpus,
            karg: self.karg.clone(),
            connect: self.connect.clone(),
        };

        // Run the upload
        crate::libvirt::upload::run(upload_opts.clone())?;

        // Return the generated volume name (with digest)
        Ok(upload_opts.get_cached_volume_name(&image_digest))
    }

    /// Parse memory size to MB
    fn parse_memory(&self) -> Result<u64> {
        crate::utils::parse_size(&self.memory).map(|bytes| bytes / (1024 * 1024))
    }

    /// Check if volume exists in the specified pool
    fn check_volume_exists(&self, volume_name: &str) -> Result<String> {
        let volume_path = if volume_name.ends_with(".raw") {
            volume_name.to_string()
        } else {
            format!("{}.raw", volume_name)
        };

        let output = self
            .virsh_command()
            .args(&["vol-info", &volume_path, "--pool", &self.pool])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!(
                "Volume '{}' not found in pool '{}': {}",
                volume_path,
                self.pool,
                stderr
            ));
        }

        // Get the full volume path
        let vol_path_output = self
            .virsh_command()
            .args(&["vol-path", &volume_path, "--pool", &self.pool])
            .output()?;

        if vol_path_output.status.success() {
            let path = String::from_utf8(vol_path_output.stdout)?;
            Ok(path.trim().to_string())
        } else {
            Err(eyre!("Failed to get volume path for '{}'", volume_path))
        }
    }

    /// Extract metadata from bootc volume
    fn get_volume_metadata(&self, volume_name: &str) -> Result<BootcVolumeMetadata> {
        let volume_path = if volume_name.ends_with(".raw") {
            volume_name.to_string()
        } else {
            format!("{}.raw", volume_name)
        };

        let output = self
            .virsh_command()
            .args(&["vol-dumpxml", &volume_path, "--pool", &self.pool])
            .output()?;
        if !output.status.success() {
            return Err(eyre!("Failed to dumpxml: {:?}", output.status));
        }

        let xml = String::from_utf8(output.stdout)?;
        debug!("Volume XML: {}", xml);

        // Parse XML to extract bootc metadata using DOM parser
        let dom = xml_utils::parse_xml_dom(&xml)
            .map_err(|e| eyre!("Failed to parse volume XML: {}", e))?;

        let source_image = dom
            .find("bootc:source-image")
            .or_else(|| dom.find("source-image"))
            .map(|node| node.text_content().to_string());

        Ok(BootcVolumeMetadata { source_image })
    }

    /// Check if domain already exists
    fn check_domain_exists(&self, domain_name: &str) -> bool {
        let output = self
            .virsh_command()
            .args(&["dominfo", domain_name])
            .output();

        match output {
            Ok(output) => output.status.success(),
            Err(_) => false,
        }
    }

    /// Create a domain-specific copy of the volume
    fn create_domain_volume(&self, source_volume_name: &str, domain_name: &str) -> Result<String> {
        let domain_volume_name = format!("{}-{}", source_volume_name, domain_name);
        let domain_volume_path = format!("{}.raw", domain_volume_name);
        let source_volume_path = format!("{}.raw", source_volume_name);

        // Check if domain volume already exists
        let check_output = self
            .virsh_command()
            .args(&["vol-info", &domain_volume_path, "--pool", &self.pool])
            .output()?;

        if check_output.status.success() {
            if self.force {
                debug!("Removing existing domain volume: {}", domain_volume_name);
                let _ = self
                    .virsh_command()
                    .args(&["vol-delete", &domain_volume_path, "--pool", &self.pool])
                    .output();
            } else {
                return Err(eyre!(
                    "Domain volume '{}' already exists. Use --force to recreate.",
                    domain_volume_name
                ));
            }
        }

        debug!(
            "Creating domain-specific volume: {} from {}",
            domain_volume_name, source_volume_name
        );

        // Clone the source volume to create a domain-specific copy
        let output = self
            .virsh_command()
            .args(&[
                "vol-clone",
                &source_volume_path,
                &domain_volume_path,
                "--pool",
                &self.pool,
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!("Failed to clone volume: {}", stderr));
        }

        // Get the path to the new domain volume
        let vol_path_output = self
            .virsh_command()
            .args(&["vol-path", &domain_volume_path, "--pool", &self.pool])
            .output()?;

        if vol_path_output.status.success() {
            let path = String::from_utf8(vol_path_output.stdout)?;
            Ok(path.trim().to_string())
        } else {
            Err(eyre!("Failed to get domain volume path"))
        }
    }

    /// Create the libvirt domain
    fn create_domain(
        &self,
        _volume_path: &str,
        metadata: &BootcVolumeMetadata,
        volume_name: &str,
    ) -> Result<()> {
        let domain_name = self.get_domain_name();
        let memory_mb = self.parse_memory()?;

        // Create a domain-specific volume copy to avoid file locking issues
        let domain_volume_path = self.create_domain_volume(volume_name, &domain_name)?;
        debug!("Using domain-specific volume: {}", domain_volume_path);

        // Setup SSH configuration
        let ssh_config = self.setup_ssh_config(&domain_name)?;

        debug!(
            "Creating domain '{}' from volume '{}' in pool '{}'",
            domain_name, volume_name, self.pool
        );

        if self.check_domain_exists(&domain_name) && !self.force {
            return Err(eyre!(
                "Domain '{}' already exists. Use --force to recreate.",
                domain_name
            ));
        }

        // If domain exists and force is specified, undefine it first
        if self.check_domain_exists(&domain_name) && self.force {
            debug!("Domain exists, removing it first (--force specified)");
            let _ = self
                .virsh_command()
                .args(&["destroy", &domain_name])
                .output();
            let _ = self
                .virsh_command()
                .args(&["undefine", &domain_name])
                .output();
        }

        if self.dry_run {
            debug!("Dry run mode - would create domain with:");
            debug!("  Name: {}", domain_name);
            debug!("  Memory: {} MB", memory_mb);
            debug!("  vCPUs: {}", self.vcpus.unwrap_or_else(default_vcpus));
            debug!("  Volume: {}", domain_volume_path);
            debug!("  Network: {}", self.network);
            if let Some(ref source_image) = metadata.source_image {
                debug!("  Source Image: {}", source_image);
            }
            return Ok(());
        }

        // Prepare QEMU args for SSH injection (if SSH is configured)
        let mut qemu_args = Vec::new();
        let network_config = if ssh_config.is_some() {
            // When SSH is configured, disable default networking to avoid conflicts
            // and use QEMU commandline for SSH port forwarding
            "none"
        } else {
            &self.network
        };

        if let Some(ref ssh) = ssh_config {
            // Generate SMBIOS credential for SSH key injection
            let smbios_cred = smbios_cred_for_root_ssh(&ssh.public_key)?;
            debug!("Injecting SSH key via SMBIOS credential");

            qemu_args.push("-smbios".to_string());
            qemu_args.push(format!("type=11,value={}", smbios_cred));

            // Add SSH port forwarding - this replaces the default network
            // Use explicit PCI address to avoid conflicts with libvirt's device management
            qemu_args.push("-netdev".to_string());
            qemu_args.push(format!("user,id=ssh0,hostfwd=tcp::{}-:22", ssh.port));
            qemu_args.push("-device".to_string());
            qemu_args.push("virtio-net-pci,netdev=ssh0,addr=0x3".to_string());
        }

        // Build domain configuration
        let mut domain_builder = DomainBuilder::new()
            .with_name(&domain_name)
            .with_memory(memory_mb)
            .with_vcpus(self.vcpus.unwrap_or_else(default_vcpus))
            .with_disk(&domain_volume_path)
            .with_network(network_config);

        // Add QEMU arguments if we have any
        if !qemu_args.is_empty() {
            domain_builder = domain_builder.with_qemu_args(qemu_args);
        }

        if self.vnc {
            let port = self
                .vnc_port
                .unwrap_or(5900 + self.vcpus.unwrap_or_else(default_vcpus) as u16);
            domain_builder = domain_builder.with_vnc(port);
        }

        if !self.karg.is_empty() {
            domain_builder = domain_builder.with_kernel_args(&self.karg.join(" "));
        }

        // Add metadata to domain
        if let Some(ref source_image) = metadata.source_image {
            domain_builder = domain_builder.with_metadata("bootc:source-image", source_image);
        }

        // Add SSH metadata if configured
        if let Some(ref ssh) = ssh_config {
            // Base64 encode the private key to avoid XML formatting issues
            let encoded_private_key = base64::engine::general_purpose::STANDARD
                .encode(ssh.private_key_content.as_bytes());

            domain_builder = domain_builder
                .with_metadata("ssh-private-key-base64", &encoded_private_key)
                .with_metadata("ssh-port", &ssh.port.to_string())
                .with_metadata("ssh-generated", &ssh.is_generated.to_string());
        }

        let domain_xml = domain_builder.build_xml()?;

        debug!("Domain XML: {}", domain_xml);

        // Define the domain
        let output = self
            .virsh_command()
            .args(&["define", "/dev/stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let mut child = output;
        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            stdin.write_all(domain_xml.as_bytes())?;
        }

        let output = child.wait_with_output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!("Failed to define domain: {}", stderr));
        }

        debug!("Domain '{}' created successfully", domain_name);

        // Start domain if requested
        if self.start {
            debug!("Starting domain '{}'", domain_name);
            let output = self
                .virsh_command()
                .args(&["start", &domain_name])
                .output()?;

            if output.status.success() {
                debug!("Domain '{}' started successfully", domain_name);

                // Show connection information
                self.show_connection_info(&domain_name, &ssh_config)?;
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!("Failed to start domain: {}", stderr);
            }
        }

        Ok(())
    }

    /// Setup SSH configuration (generate keys or load existing)
    fn setup_ssh_config(&self, domain_name: &str) -> Result<Option<SshConfig>> {
        if !self.generate_ssh_key && self.ssh_key.is_none() {
            return Ok(None);
        }

        let port = self.find_available_ssh_port();

        if let Some(ref ssh_key_path) = self.ssh_key {
            // Use existing SSH key
            let private_key_path = Utf8PathBuf::from(ssh_key_path);
            let public_key_path = format!("{}.pub", ssh_key_path);

            if !private_key_path.exists() {
                return Err(eyre!("SSH private key not found: {}", ssh_key_path));
            }

            if !Utf8Path::new(&public_key_path).exists() {
                return Err(eyre!("SSH public key not found: {}", public_key_path));
            }

            let private_key_content = fs::read_to_string(private_key_path.as_std_path())
                .map_err(|e| eyre!("Failed to read private key {}: {}", ssh_key_path, e))?;

            let public_key = fs::read_to_string(&public_key_path)
                .map_err(|e| eyre!("Failed to read public key {}: {}", public_key_path, e))?;

            debug!("Using existing SSH key: {}", ssh_key_path);

            Ok(Some(SshConfig {
                private_key_content: private_key_content.trim().to_string(),
                public_key: public_key.trim().to_string(),
                port,
                is_generated: false,
            }))
        } else if self.generate_ssh_key {
            // Generate ephemeral SSH keys (in memory, will be stored in domain XML)
            debug!(
                "Generating ephemeral SSH keypair for domain '{}'",
                domain_name
            );

            // Use temporary files for key generation, then read content and clean up
            let temp_dir = tempfile::tempdir()
                .map_err(|e| eyre!("Failed to create temporary directory: {}", e))?;

            // Generate keypair
            let keypair = generate_ssh_keypair(
                camino::Utf8Path::from_path(temp_dir.path()).unwrap(),
                "id_rsa",
            )?;

            // Read the key contents from the generated keypair
            let private_key_content = fs::read_to_string(keypair.private_key_path.as_std_path())
                .map_err(|e| eyre!("Failed to read generated private key: {}", e))?;

            let public_key = fs::read_to_string(keypair.public_key_path.as_std_path())
                .map_err(|e| eyre!("Failed to read generated public key: {}", e))?;

            debug!("Generated ephemeral SSH keypair (will be stored in domain XML)");

            // temp_dir will be automatically cleaned up when dropped

            Ok(Some(SshConfig {
                private_key_content: private_key_content.trim().to_string(),
                public_key: public_key.trim().to_string(),
                port,
                is_generated: true,
            }))
        } else {
            Ok(None)
        }
    }

    /// Find an available SSH port for port forwarding
    fn find_available_ssh_port(&self) -> u16 {
        self.ssh_port.unwrap_or_else(|| {
            // Start from 2222 and find first available port
            for port in 2222..3000 {
                if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
                    return port;
                }
            }
            2222 // Fallback
        })
    }

    /// Display connection information for the created domain
    fn show_connection_info(
        &self,
        domain_name: &str,
        ssh_config: &Option<SshConfig>,
    ) -> Result<()> {
        debug!("Domain '{}' connection information:", domain_name);

        if self.vnc {
            let port = self
                .vnc_port
                .unwrap_or(5900 + self.vcpus.unwrap_or_else(default_vcpus) as u16);
            debug!("  VNC Console: vnc://localhost:{}", port);
        }

        debug!("  Serial Console: virsh console {}", domain_name);

        if let Some(ref ssh) = ssh_config {
            debug!("  SSH Access: bcvk libvirt ssh {}", domain_name);
            debug!("  SSH Port: {}", ssh.port);
            if ssh.is_generated {
                debug!("  SSH Key: stored in domain XML (ephemeral)");
            } else {
                debug!("  SSH Key: imported from existing key");
            }
        }

        Ok(())
    }
}

/// Execute the libvirt domain creation process
pub fn run(opts: LibvirtCreateOpts) -> Result<()> {
    debug!(
        "Creating libvirt domain from: {}",
        opts.volume_name_or_image
    );

    // Phase 1: Ensure volume exists (auto-upload if needed)
    let volume_name = opts.ensure_volume_exists()?;

    // Phase 2: Validate volume exists and get path
    let volume_path = opts.check_volume_exists(&volume_name)?;
    debug!("Found volume at: {}", volume_path);

    // Phase 3: Extract volume metadata
    let metadata = opts.get_volume_metadata(&volume_name)?;
    if let Some(ref source_image) = metadata.source_image {
        debug!("Volume contains bootc image: {}", source_image);
    }

    // Phase 4: Create and optionally start domain
    opts.create_domain(&volume_path, &metadata, &volume_name)?;

    Ok(())
}
