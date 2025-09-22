//! libvirt run command - run a bootable container as a persistent VM
//!
//! This module provides the core functionality for creating and managing
//! libvirt-based VMs from bootc container images.

use camino::{Utf8Path, Utf8PathBuf};
use clap::Parser;
use color_eyre::{eyre::Context, Result};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};

use crate::common_opts::MemoryOpts;
use crate::domain_list::DomainLister;
use crate::libvirt::domain::VirtiofsFilesystem;
use crate::utils::parse_memory_to_mb;

/// Options for creating and running a bootable container VM
#[derive(Debug, Parser)]
pub struct LibvirtRunOpts {
    /// Container image to run as a bootable VM
    pub image: String,

    /// Name for the VM (auto-generated if not specified)
    #[clap(long)]
    pub name: Option<String>,

    #[clap(flatten)]
    pub memory: MemoryOpts,

    /// Number of virtual CPUs for the VM
    #[clap(long, default_value = "2")]
    pub cpus: u32,

    /// Disk size for the VM (e.g. 20G, 10240M, or plain number for bytes)
    #[clap(long, default_value = "20G")]
    pub disk_size: String,

    /// Root filesystem type for installation
    #[clap(long, default_value = "ext4")]
    pub filesystem: String,

    /// Port mapping from host to VM
    #[clap(long = "port", short = 'p', action = clap::ArgAction::Append)]
    pub port_mappings: Vec<String>,

    /// Volume mount from host to VM
    #[clap(long = "volume", short = 'v', action = clap::ArgAction::Append)]
    pub volumes: Vec<String>,

    /// Network mode for the VM
    #[clap(long, default_value = "user")]
    pub network: String,

    /// Keep the VM running in background after creation
    #[clap(long)]
    pub detach: bool,

    /// Automatically SSH into the VM after creation
    #[clap(long)]
    pub ssh: bool,

    /// Mount host container storage (RO) at /run/virtiofs-mnt-hoststorage
    #[clap(long = "bind-storage-ro")]
    pub bind_storage_ro: bool,
}

/// Execute the libvirt run command
pub fn run(opts: LibvirtRunOpts) -> Result<()> {
    run_vm_impl(opts)
}

/// Create and run a bootable container VM (implementation)
pub fn run_vm_impl(opts: LibvirtRunOpts) -> Result<()> {
    use crate::install_options::InstallOptions;
    use crate::run_ephemeral::CommonVmOpts;
    use crate::to_disk::ToDiskOpts;

    let lister = DomainLister::new();
    let existing_domains = lister
        .list_all_domains()
        .with_context(|| "Failed to list existing domains")?;

    // Generate or validate VM name
    let vm_name = match &opts.name {
        Some(name) => {
            if existing_domains.contains(name) {
                return Err(color_eyre::eyre::eyre!("VM '{}' already exists", name));
            }
            name.clone()
        }
        None => generate_unique_vm_name(&opts.image, &existing_domains),
    };

    println!(
        "Creating libvirt domain '{}' (install source container image: {})",
        vm_name, opts.image
    );

    // Create disk path in the standard libvirt images directory
    let disk_path =
        create_disk_path(&vm_name, &opts.image).with_context(|| "Failed to create disk path")?;

    // Phase 1: Create bootable disk image using to_disk
    println!("📀 Creating bootable disk image...");

    let to_disk_opts = ToDiskOpts {
        source_image: opts.image.clone(),
        target_disk: disk_path.clone(),
        disk_size: Some(opts.disk_size.clone()),
        format: crate::to_disk::Format::Raw, // Default to raw format
        install: InstallOptions {
            filesystem: Some(opts.filesystem.clone()),
            root_size: None,
            storage_path: None,
        },
        common: CommonVmOpts {
            memory: opts.memory.clone(),
            vcpus: Some(opts.cpus),
            kernel_args: vec![],
            net: None,
            console: false,
            debug: false,
            virtio_serial_out: vec![],
            execute: vec![],
            ssh_keygen: true, // Enable SSH key generation
        },
        label: vec![],
    };

    // Run the disk creation
    crate::to_disk::run(to_disk_opts).with_context(|| "Failed to create bootable disk image")?;

    println!("Disk image created at: {}", disk_path);

    // Phase 2: Create libvirt domain
    println!("Creating libvirt domain...");

    // Create the domain directly (simpler than using libvirt/create for files)
    create_libvirt_domain_from_disk(&vm_name, &disk_path, &opts)
        .with_context(|| "Failed to create libvirt domain")?;

    // VM is now managed by libvirt, no need to track separately

    println!("VM '{}' created successfully!", vm_name);
    println!("  Image: {}", opts.image);
    println!("  Disk: {}", disk_path);
    println!("  Memory: {}", opts.memory.memory);
    println!("  CPUs: {}", opts.cpus);

    if opts.ssh {
        // Use the libvirt SSH functionality directly
        let ssh_opts = crate::libvirt::ssh::LibvirtSshOpts {
            domain_name: vm_name,
            connect: None,
            user: "root".to_string(),
            command: vec![],
            strict_host_keys: false,
            timeout: 30,
        };
        crate::libvirt::ssh::run(ssh_opts)
    } else {
        println!("\nUse 'bcvk libvirt ssh {}' to connect", vm_name);
        Ok(())
    }
}

/// Get the path of the default libvirt storage pool
fn get_libvirt_storage_pool_path() -> Result<Utf8PathBuf> {
    use std::process::Command;

    // Try user session first (qemu:///session)
    let output = Command::new("virsh")
        .args(&["-c", "qemu:///session", "pool-dumpxml", "default"])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => {
            // Try system session (qemu:///system)
            Command::new("virsh")
                .args(&["-c", "qemu:///system", "pool-dumpxml", "default"])
                .output()
                .with_context(|| "Failed to query libvirt storage pool")?
        }
    };

    if !output.status.success() {
        return Err(color_eyre::eyre::eyre!(
            "Failed to get default storage pool info"
        ));
    }

    let xml = String::from_utf8(output.stdout).with_context(|| "Invalid UTF-8 in virsh output")?;

    // Extract path from XML
    // Looking for: <path>/some/path</path>
    let start_tag = "<path>";
    let end_tag = "</path>";

    if let Some(start_pos) = xml.find(start_tag) {
        let start = start_pos + start_tag.len();
        if let Some(end_pos) = xml[start..].find(end_tag) {
            let path_str = &xml[start..start + end_pos];
            return Ok(Utf8PathBuf::from(path_str.trim()));
        }
    }

    Err(color_eyre::eyre::eyre!(
        "Could not find path in storage pool XML"
    ))
}

/// Generate a unique VM name from an image name
fn generate_unique_vm_name(image: &str, existing_domains: &[String]) -> String {
    // Extract image name from full image path
    let base_name = if let Some(last_slash) = image.rfind('/') {
        &image[last_slash + 1..]
    } else {
        image
    };

    // Remove tag if present
    let base_name = if let Some(colon) = base_name.find(':') {
        &base_name[..colon]
    } else {
        base_name
    };

    // Sanitize name (replace invalid characters with hyphens)
    let sanitized: String = base_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();

    // Find unique name by appending numbers
    let mut candidate = sanitized.clone();
    let mut counter = 1;

    while existing_domains.contains(&candidate) {
        counter += 1;
        candidate = format!("{}-{}", sanitized, counter);
    }

    candidate
}

/// Create disk path for a VM using image hash as suffix
fn create_disk_path(vm_name: &str, source_image: &str) -> Result<Utf8PathBuf> {
    // Query libvirt for the default storage pool path
    let base_dir = get_libvirt_storage_pool_path().unwrap_or_else(|_| {
        // Fallback to standard paths if we can't query libvirt
        if let Ok(home) = std::env::var("HOME") {
            Utf8PathBuf::from(home).join(".local/share/libvirt/images")
        } else {
            Utf8PathBuf::from("/var/lib/libvirt/images")
        }
    });

    // Ensure the directory exists
    fs::create_dir_all(base_dir.as_std_path())
        .with_context(|| format!("Failed to create directory: {:?}", base_dir))?;

    // Generate a hash of the source image for uniqueness
    let mut hasher = DefaultHasher::new();
    source_image.hash(&mut hasher);
    let image_hash = hasher.finish();
    let hash_prefix = format!("{:x}", image_hash)
        .chars()
        .take(8)
        .collect::<String>();

    // Try to find a unique filename
    let mut counter = 0;
    loop {
        let disk_name = if counter == 0 {
            format!("{}-{}.raw", vm_name, hash_prefix)
        } else {
            format!("{}-{}-{}.raw", vm_name, hash_prefix, counter)
        };

        let disk_path = base_dir.join(&disk_name);

        // Check if file exists
        if !disk_path.exists() {
            return Ok(disk_path);
        }

        counter += 1;
        if counter > 100 {
            return Err(color_eyre::eyre::eyre!(
                "Could not create unique disk path after 100 attempts"
            ));
        }
    }
}

/// Find an available SSH port for port forwarding using random allocation
fn find_available_ssh_port() -> u16 {
    use rand::Rng;

    // Try random ports in the range 2222-3000 to avoid conflicts in concurrent scenarios
    let mut rng = rand::rng();
    const PORT_RANGE_START: u16 = 2222;
    const PORT_RANGE_END: u16 = 3000;

    // Try up to 100 random attempts
    for _ in 0..100 {
        let port = rng.random_range(PORT_RANGE_START..PORT_RANGE_END);
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }

    // Fallback to sequential search if random allocation fails
    for port in PORT_RANGE_START..PORT_RANGE_END {
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }

    PORT_RANGE_START // Ultimate fallback
}

/// Create a libvirt domain directly from a disk image file
fn create_libvirt_domain_from_disk(
    domain_name: &str,
    disk_path: &Utf8Path,
    opts: &LibvirtRunOpts,
) -> Result<()> {
    use crate::libvirt::domain::DomainBuilder;
    use crate::ssh::generate_ssh_keypair;
    use crate::sshcred::smbios_cred_for_root_ssh;
    use std::process::Command;
    use tracing::debug;

    // Generate SSH keypair for the domain
    debug!(
        "Generating ephemeral SSH keypair for domain '{}'",
        domain_name
    );

    // Find available SSH port for this domain
    let ssh_port = find_available_ssh_port();
    debug!(
        "Allocated SSH port {} for domain '{}'",
        ssh_port, domain_name
    );

    // Use temporary files for key generation, then read content and clean up
    let temp_dir = tempfile::tempdir()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to create temporary directory: {}", e))?;

    // Generate keypair
    let keypair = generate_ssh_keypair(
        camino::Utf8Path::from_path(temp_dir.path()).unwrap(),
        "id_rsa",
    )?;

    // Read the key contents from the generated keypair
    let private_key_content = std::fs::read_to_string(&keypair.private_key_path)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to read generated private key: {}", e))?;
    let public_key_content = std::fs::read_to_string(&keypair.public_key_path)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to read generated public key: {}", e))?;

    let private_key_base64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        private_key_content.as_bytes(),
    );
    debug!("Generated ephemeral SSH keypair (will be stored in domain XML)");

    // Generate SMBIOS credential for SSH key injection
    let smbios_cred = smbios_cred_for_root_ssh(&public_key_content)?;

    let memory = parse_memory_to_mb(&opts.memory.memory)?;

    // Build domain XML using the existing DomainBuilder with bootc metadata and SSH keys
    let mut domain_builder = DomainBuilder::new()
        .with_name(domain_name)
        .with_memory(memory.into())
        .with_vcpus(opts.cpus)
        .with_disk(disk_path.as_str())
        .with_network("none") // Use QEMU args for SSH networking instead
        .with_metadata("bootc:source-image", &opts.image)
        .with_metadata("bootc:memory-mb", &opts.memory.to_string())
        .with_metadata("bootc:vcpus", &opts.cpus.to_string())
        .with_metadata("bootc:disk-size-gb", &opts.disk_size.to_string())
        .with_metadata("bootc:filesystem", &opts.filesystem)
        .with_metadata("bootc:network", &opts.network)
        .with_metadata("bootc:ssh-generated", "true")
        .with_metadata("bootc:ssh-private-key-base64", &private_key_base64)
        .with_metadata("bootc:ssh-port", &ssh_port.to_string());

    // Add container storage mount if requested
    if opts.bind_storage_ro {
        let storage_path = crate::utils::detect_container_storage_path()
            .context("Failed to detect container storage path.")?;
        crate::utils::validate_container_storage_path(&storage_path)
            .context("Container storage validation failed")?;

        debug!(
            "Adding container storage from {} as hoststorage virtiofs mount",
            storage_path
        );

        let virtiofs_fs = VirtiofsFilesystem {
            source_dir: storage_path.to_string(),
            tag: "hoststorage".to_string(),
            readonly: true,
        };

        domain_builder = domain_builder
            .with_virtiofs_filesystem(virtiofs_fs)
            .with_metadata("bootc:bind-storage-ro", "true")
            .with_metadata("bootc:storage-path", storage_path.as_str());
    }

    let domain_xml = domain_builder
        .with_qemu_args(vec![
            "-smbios".to_string(),
            format!("type=11,value={}", smbios_cred),
            "-netdev".to_string(),
            format!("user,id=ssh0,hostfwd=tcp::{}-:22", ssh_port),
            "-device".to_string(),
            "virtio-net-pci,netdev=ssh0,addr=0x3".to_string(),
        ])
        .build_xml()
        .with_context(|| "Failed to build domain XML")?;

    // Write XML to temporary file
    let xml_path = format!("/tmp/{}.xml", domain_name);
    std::fs::write(&xml_path, domain_xml).with_context(|| "Failed to write domain XML")?;

    // Define the domain
    let output = Command::new("virsh")
        .args(&["define", &xml_path])
        .output()
        .with_context(|| "Failed to run virsh define")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(color_eyre::eyre::eyre!(
            "Failed to define libvirt domain: {}",
            stderr
        ));
    }

    // Start the domain by default (compatibility)
    let output = Command::new("virsh")
        .args(&["start", domain_name])
        .output()
        .with_context(|| "Failed to start domain")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(color_eyre::eyre::eyre!(
            "Failed to start libvirt domain: {}",
            stderr
        ));
    }

    // Clean up temporary XML file
    let _ = std::fs::remove_file(&xml_path);

    Ok(())
}
