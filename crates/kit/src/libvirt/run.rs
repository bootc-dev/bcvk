//! libvirt run command - run a bootable container as a persistent VM
//!
//! This module provides the core functionality for creating and managing
//! libvirt-based VMs from bootc container images.

use camino::{Utf8Path, Utf8PathBuf};
use clap::{Parser, ValueEnum};
use color_eyre::eyre;
use color_eyre::{eyre::Context, Result};
use std::fs;
use tracing::{debug, info};

use crate::common_opts::MemoryOpts;
use crate::domain_list::DomainLister;
use crate::install_options::InstallOptions;
use crate::libvirt::domain::VirtiofsFilesystem;
use crate::utils::parse_memory_to_mb;
use crate::xml_utils;

/// Firmware type for virtual machines
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum FirmwareType {
    /// UEFI with secure boot enabled (default)
    UefiSecure,
    /// UEFI with secure boot explicitly disabled
    UefiInsecure,
    /// Legacy BIOS
    Bios,
}

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

    /// Installation options (filesystem, root-size, etc.)
    #[clap(flatten)]
    pub install: InstallOptions,

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

    /// Firmware type for the VM (defaults to uefi-secure)
    #[clap(long, default_value = "uefi-secure")]
    pub firmware: FirmwareType,

    /// Disable TPM 2.0 support (enabled by default)
    #[clap(long)]
    pub disable_tpm: bool,

    /// Directory containing secure boot keys (required for uefi-secure)
    #[clap(long)]
    pub secure_boot_keys: Option<Utf8PathBuf>,
}

/// Execute the libvirt run command
pub fn run(global_opts: &crate::libvirt::LibvirtOptions, opts: LibvirtRunOpts) -> Result<()> {
    use crate::images;

    let connect_uri = global_opts.connect.as_ref();
    let lister = match connect_uri {
        Some(uri) => DomainLister::with_connection(uri.clone()),
        None => DomainLister::new(),
    };
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

    // Get the image digest for caching
    let inspect = images::inspect(&opts.image)?;
    let image_digest = inspect.digest.to_string();
    debug!("Image digest: {}", image_digest);

    // Phase 1: Find or create a base disk image
    let base_disk_path = crate::libvirt::base_disks::find_or_create_base_disk(
        &opts.image,
        &image_digest,
        &opts.install,
        &[], // kernel_args
        connect_uri,
    )
    .with_context(|| "Failed to find or create base disk")?;

    println!("Using base disk image: {}", base_disk_path);

    // Phase 2: Clone the base disk to create a VM-specific disk
    let disk_path =
        crate::libvirt::base_disks::clone_from_base(&base_disk_path, &vm_name, connect_uri)
            .with_context(|| "Failed to clone VM disk from base")?;

    println!("Created VM disk: {}", disk_path);

    // Phase 3: Create libvirt domain
    println!("Creating libvirt domain...");

    // Create the domain directly (simpler than using libvirt/create for files)
    create_libvirt_domain_from_disk(&vm_name, &disk_path, &image_digest, &opts, global_opts)
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
            user: "root".to_string(),
            command: vec![],
            strict_host_keys: false,
            timeout: 30,
            log_level: "ERROR".to_string(),
            extra_options: vec![],
        };
        crate::libvirt::ssh::run(global_opts, ssh_opts)
    } else {
        println!("\nUse 'bcvk libvirt ssh {}' to connect", vm_name);
        Ok(())
    }
}

/// Determine the appropriate default storage pool path based on connection type
fn get_default_pool_path(connect_uri: &str) -> Utf8PathBuf {
    if connect_uri.contains("/session") {
        // User session: use XDG_DATA_HOME or default to ~/.local/share/libvirt/images
        let data_home = std::env::var("XDG_DATA_HOME")
            .ok()
            .map(Utf8PathBuf::from)
            .unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                Utf8PathBuf::from(home).join(".local/share")
            });
        data_home.join("libvirt/images")
    } else {
        // System session: use /var/lib/libvirt/images
        Utf8PathBuf::from("/var/lib/libvirt/images")
    }
}

/// Ensure the default libvirt storage pool exists, creating it if necessary
fn ensure_default_pool(connect_uri: &str) -> Result<()> {
    // Check if default pool already exists
    let mut cmd = crate::hostexec::command("virsh", None)?;
    cmd.args(&["-c", connect_uri, "pool-info", "default"]);
    let output = cmd
        .output()
        .with_context(|| "Failed to check for default pool")?;

    if output.status.success() {
        // Pool exists, make sure it's active
        let mut cmd = crate::hostexec::command("virsh", None)?;
        cmd.args(&["-c", connect_uri, "pool-start", "default"]);
        let _ = cmd.output(); // Ignore errors if already started
        return Ok(());
    }

    // Pool doesn't exist, need to create it
    let pool_path = get_default_pool_path(connect_uri);
    info!("Creating default storage pool at {:?}", pool_path);

    // Create the directory if it doesn't exist
    fs::create_dir_all(&pool_path)
        .with_context(|| format!("Failed to create pool directory: {:?}", pool_path))?;

    // Create pool XML
    let pool_xml = format!(
        r#"<pool type='dir'>
  <name>default</name>
  <target>
    <path>{}</path>
  </target>
</pool>"#,
        pool_path
    );

    // Write XML to temporary file
    let xml_path = "/tmp/default-pool.xml";
    std::fs::write(xml_path, &pool_xml).with_context(|| "Failed to write pool XML")?;

    // Define the pool
    let mut cmd = crate::hostexec::command("virsh", None)?;
    cmd.args(&["-c", connect_uri, "pool-define", xml_path]);
    let output = cmd.output().with_context(|| "Failed to define pool")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = std::fs::remove_file(xml_path);
        return Err(color_eyre::eyre::eyre!(
            "Failed to define default pool: {}",
            stderr
        ));
    }

    // Build the pool (creates directory structure)
    let mut cmd = crate::hostexec::command("virsh", None)?;
    cmd.args(&["-c", connect_uri, "pool-build", "default"]);
    let _ = cmd.output(); // Directory might already exist

    // Start the pool
    let mut cmd = crate::hostexec::command("virsh", None)?;
    cmd.args(&["-c", connect_uri, "pool-start", "default"]);
    let output = cmd.output().with_context(|| "Failed to start pool")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = std::fs::remove_file(xml_path);
        return Err(color_eyre::eyre::eyre!(
            "Failed to start default pool: {}",
            stderr
        ));
    }

    // Autostart the pool
    let mut cmd = crate::hostexec::command("virsh", None)?;
    cmd.args(&["-c", connect_uri, "pool-autostart", "default"]);
    let _ = cmd.output(); // Not critical if this fails

    // Clean up temporary XML file
    let _ = std::fs::remove_file(xml_path);

    info!("Default storage pool created successfully");
    Ok(())
}

/// Get the path of the default libvirt storage pool
pub fn get_libvirt_storage_pool_path(connect_uri: Option<&String>) -> Result<Utf8PathBuf> {
    // If a specific connection URI is provided, use it
    if let Some(uri) = connect_uri {
        // Ensure pool exists before querying
        ensure_default_pool(uri)?;

        let mut cmd = crate::hostexec::command("virsh", None)?;
        cmd.args(&["-c", uri, "pool-dumpxml", "default"]);
        let output = cmd
            .output()
            .with_context(|| "Failed to query libvirt storage pool")?;

        if output.status.success() {
            let xml = String::from_utf8(output.stdout)
                .with_context(|| "Invalid UTF-8 in virsh output")?;
            let dom = xml_utils::parse_xml_dom(&xml)
                .with_context(|| "Failed to parse storage pool XML")?;

            if let Some(path_node) = dom.find("path") {
                let path_str = path_node.text_content().trim();
                if !path_str.is_empty() {
                    return Ok(Utf8PathBuf::from(path_str));
                }
            }
        }
    }

    // Try user session first (qemu:///session)
    let session_uri = "qemu:///session";
    ensure_default_pool(session_uri)?;

    let mut cmd = crate::hostexec::command("virsh", None)?;
    cmd.args(&["-c", session_uri, "pool-dumpxml", "default"]);
    let output = cmd.output();

    let (output, uri_used) = match output {
        Ok(o) if o.status.success() => (o, session_uri),
        _ => {
            // Try system session (qemu:///system)
            let system_uri = "qemu:///system";
            ensure_default_pool(system_uri)?;

            let mut cmd = crate::hostexec::command("virsh", None)?;
            cmd.args(&["-c", system_uri, "pool-dumpxml", "default"]);
            let out = cmd
                .output()
                .with_context(|| "Failed to query libvirt storage pool")?;
            (out, system_uri)
        }
    };

    if !output.status.success() {
        return Err(color_eyre::eyre::eyre!(
            "Failed to get default storage pool info for {}",
            uri_used
        ));
    }

    let xml = String::from_utf8(output.stdout).with_context(|| "Invalid UTF-8 in virsh output")?;

    // Parse XML using DOM parser and extract path element
    let dom = xml_utils::parse_xml_dom(&xml).with_context(|| "Failed to parse storage pool XML")?;

    if let Some(path_node) = dom.find("path") {
        let path_str = path_node.text_content().trim();
        if !path_str.is_empty() {
            return Ok(Utf8PathBuf::from(path_str));
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

/// List all volumes in the default storage pool
pub fn list_storage_pool_volumes(connect_uri: Option<&String>) -> Result<Vec<Utf8PathBuf>> {
    // Get the storage pool path from XML
    let pool_path = get_libvirt_storage_pool_path(connect_uri)?;

    debug!("Scanning storage pool directory: {:?}", pool_path);

    let mut volumes = Vec::new();

    // Read directory and collect volume files
    if let Ok(entries) = fs::read_dir(&pool_path) {
        for entry in entries.flatten() {
            if let Ok(path) = entry.path().into_os_string().into_string() {
                // Filter for disk image files
                if path.ends_with(".raw") || path.ends_with(".qcow2") {
                    volumes.push(Utf8PathBuf::from(path));
                }
            }
        }
    }

    debug!("Found {} volumes in storage pool", volumes.len());
    Ok(volumes)
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

/// Check if the libvirt version supports readonly virtiofs filesystems
/// Requires libvirt 11.0+ and modern QEMU with rust-based virtiofsd
fn check_libvirt_readonly_support() -> Result<()> {
    let version = crate::libvirt::status::parse_libvirt_version()
        .with_context(|| "Failed to check libvirt version")?;

    if crate::libvirt::status::supports_readonly_virtiofs(&version) {
        Ok(())
    } else {
        match version {
            Some(v) => Err(color_eyre::eyre::eyre!(
                "The --bind-storage-ro flag requires libvirt 11.0 or later for readonly virtiofs support. \
                Current version: {}", 
                v.full_version
            )),
            None => Err(color_eyre::eyre::eyre!(
                "Could not parse libvirt version. \
                The --bind-storage-ro flag requires libvirt 11.0+ with rust-based virtiofsd support. \
                Please ensure you have a compatible libvirt version installed."
            ))
        }
    }
}

/// Create a libvirt domain directly from a disk image file
fn create_libvirt_domain_from_disk(
    domain_name: &str,
    disk_path: &Utf8Path,
    image_digest: &str,
    opts: &LibvirtRunOpts,
    global_opts: &crate::libvirt::LibvirtOptions,
) -> Result<()> {
    use crate::libvirt::domain::DomainBuilder;
    use crate::ssh::generate_ssh_keypair;
    use crate::sshcred::smbios_cred_for_root_ssh;

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

    // Setup secure boot if requested
    let secure_boot_config = if let Some(keys) = opts.secure_boot_keys.as_deref() {
        use crate::libvirt::secureboot;

        eyre::ensure!(opts.firmware == FirmwareType::UefiSecure);

        info!("Setting up secure boot configuration from {}", keys);
        let config = secureboot::setup_secure_boot(&keys).context("Failed to setup secure boot")?;
        Some(config)
    } else {
        None
    };

    // Build domain XML using the existing DomainBuilder with bootc metadata and SSH keys
    let mut domain_builder = DomainBuilder::new()
        .with_name(domain_name)
        .with_memory(memory.into())
        .with_vcpus(opts.cpus)
        .with_disk(disk_path.as_str())
        .with_network("none") // Use QEMU args for SSH networking instead
        .with_firmware(opts.firmware)
        .with_tpm(!opts.disable_tpm)
        .with_metadata("bootc:source-image", &opts.image)
        .with_metadata("bootc:memory-mb", &opts.memory.to_string())
        .with_metadata("bootc:vcpus", &opts.cpus.to_string())
        .with_metadata("bootc:disk-size-gb", &opts.disk_size.to_string())
        .with_metadata(
            "bootc:filesystem",
            opts.install
                .filesystem
                .as_ref()
                .unwrap_or(&"ext4".to_string()),
        )
        .with_metadata("bootc:network", &opts.network)
        .with_metadata("bootc:ssh-generated", "true")
        .with_metadata("bootc:ssh-private-key-base64", &private_key_base64)
        .with_metadata("bootc:ssh-port", &ssh_port.to_string())
        .with_metadata("bootc:image-digest", image_digest);

    // Add secure boot configuration if enabled
    if let Some(ref sb_config) = secure_boot_config {
        let ovmf_code = crate::libvirt::secureboot::find_ovmf_code_secboot()
            .context("Failed to find OVMF_CODE.secboot.fd")?;
        domain_builder = domain_builder
            .with_ovmf_code_path(ovmf_code.as_str())
            .with_nvram_template(sb_config.vars_template.as_str());

        // Add secure boot keys path to metadata for reference
        domain_builder =
            domain_builder.with_metadata("bootc:secure-boot-keys", sb_config.key_dir.as_str());
    }

    // Add container storage mount if requested
    if opts.bind_storage_ro {
        // Check libvirt version compatibility for readonly virtiofs
        check_libvirt_readonly_support().context("libvirt version compatibility check failed")?;

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
    let output = global_opts
        .virsh_command()
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
    let output = global_opts
        .virsh_command()
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
