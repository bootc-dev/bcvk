//! libvirt run command - run a bootable container as a VM
//!
//! This module provides the core functionality for creating and managing
//! libvirt-based VMs from bootc container images. Supports both persistent
//! VMs (survive shutdown) and transient VMs (disappear on shutdown).

use camino::{Utf8Path, Utf8PathBuf};
use clap::{Parser, ValueEnum};
use color_eyre::eyre;
use color_eyre::{eyre::Context, Result};
use std::fs;
use std::str::FromStr;
use tracing::{debug, info};

use crate::common_opts::MemoryOpts;
use crate::domain_list::DomainLister;
use crate::install_options::InstallOptions;
use crate::libvirt::domain::VirtiofsFilesystem;
use crate::utils::parse_memory_to_mb;
use crate::xml_utils;

/// Create a virsh command with optional connection URI
pub(super) fn virsh_command(connect_uri: Option<&str>) -> Result<std::process::Command> {
    let mut cmd = crate::hostexec::command("virsh", None)?;
    if let Some(uri) = connect_uri {
        cmd.arg("-c").arg(uri);
    }
    Ok(cmd)
}

/// Run a virsh command and handle errors consistently
pub(crate) fn run_virsh_cmd(connect_uri: Option<&str>, args: &[&str], err_msg: &str) -> Result<()> {
    let output = virsh_command(connect_uri)?
        .args(args)
        .output()
        .with_context(|| format!("Failed to run virsh command: {:?}", args))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(color_eyre::eyre::eyre!("{}: {}", err_msg, stderr));
    }
    Ok(())
}

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

/// Port mapping from host to VM
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortMapping {
    pub host_port: u16,
    pub guest_port: u16,
}

impl FromStr for PortMapping {
    type Err = color_eyre::Report;

    fn from_str(s: &str) -> Result<Self> {
        let (host_part, guest_part) = s.split_once(':').ok_or_else(|| {
            color_eyre::eyre::eyre!(
                "Invalid port format '{}'. Expected format: host_port:guest_port",
                s
            )
        })?;

        let host_port = host_part.trim().parse::<u16>().map_err(|_| {
            color_eyre::eyre::eyre!(
                "Invalid host port '{}'. Must be a number between 1 and 65535",
                host_part
            )
        })?;

        let guest_port = guest_part.trim().parse::<u16>().map_err(|_| {
            color_eyre::eyre::eyre!(
                "Invalid guest port '{}'. Must be a number between 1 and 65535",
                guest_part
            )
        })?;

        Ok(PortMapping {
            host_port,
            guest_port,
        })
    }
}

impl std::fmt::Display for PortMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.host_port, self.guest_port)
    }
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

    /// Port mapping from host to VM (format: host_port:guest_port, e.g., 8080:80)
    #[clap(long = "port", short = 'p', action = clap::ArgAction::Append)]
    pub port_mappings: Vec<PortMapping>,

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

    /// User-defined labels for organizing VMs (comma not allowed in labels)
    #[clap(long)]
    pub label: Vec<String>,

    /// Create a transient VM that disappears on shutdown/reboot
    #[clap(long)]
    pub transient: bool,
}

impl LibvirtRunOpts {
    /// Validate that labels don't contain commas
    fn validate_labels(&self) -> Result<()> {
        for label in &self.label {
            if label.contains(',') {
                return Err(eyre::eyre!(
                    "Label '{}' contains comma which is not allowed",
                    label
                ));
            }
        }
        Ok(())
    }
}

/// Execute the libvirt run command
pub fn run(global_opts: &crate::libvirt::LibvirtOptions, opts: LibvirtRunOpts) -> Result<()> {
    use crate::images;

    // Validate labels don't contain commas
    opts.validate_labels()?;

    let connect_uri = global_opts.connect.as_deref();
    let lister = match global_opts.connect.as_ref() {
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
        connect_uri,
    )
    .with_context(|| "Failed to find or create base disk")?;

    println!("Using base disk image: {}", base_disk_path);

    // Phase 2: Clone the base disk to create a VM-specific disk (or use base directly if transient)
    let disk_path = if opts.transient {
        println!("Transient mode: using base disk directly with overlay");
        base_disk_path
    } else {
        let cloned_disk =
            crate::libvirt::base_disks::clone_from_base(&base_disk_path, &vm_name, connect_uri)
                .with_context(|| "Failed to clone VM disk from base")?;
        println!("Created VM disk: {}", cloned_disk);
        cloned_disk
    };

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

    // Display volume mount information if any
    if !opts.volumes.is_empty() {
        println!("\nVolume mounts:");
        for volume_str in opts.volumes.iter() {
            if let Ok((host_path, tag)) = parse_volume_mount(volume_str) {
                println!(
                    "  {} (tag: {}, mount with: mount -t virtiofs {} /your/mount/point)",
                    host_path, tag, tag
                );
            }
        }
    }

    // Display port forwarding information if any
    if !opts.port_mappings.is_empty() {
        println!("\nPort forwarding:");
        for mapping in opts.port_mappings.iter() {
            println!(
                "  localhost:{} -> VM:{}",
                mapping.host_port, mapping.guest_port
            );
        }
    }

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
fn ensure_default_pool(connect_uri: Option<&str>) -> Result<()> {
    // Check if default pool already exists
    let mut cmd = virsh_command(connect_uri)?;
    cmd.args(&["pool-info", "default"]);
    let output = cmd
        .output()
        .with_context(|| "Failed to check for default pool")?;

    if output.status.success() {
        // Pool exists, make sure it's active
        let mut cmd = virsh_command(connect_uri)?;
        cmd.args(&["pool-start", "default"]);
        let _ = cmd.output(); // Ignore errors if already started
        return Ok(());
    }

    // Pool doesn't exist, need to create it
    // Determine the appropriate pool path based on the connection URI
    let pool_path = if let Some(uri) = connect_uri {
        get_default_pool_path(uri)
    } else {
        // If no URI specified, virsh will use its default connection
        // We need to query what that is to determine the appropriate pool path
        let mut cmd = virsh_command(None)?;
        cmd.args(&["uri"]);
        let output = cmd
            .output()
            .with_context(|| "Failed to query default libvirt URI")?;
        let uri_str = String::from_utf8(output.stdout)
            .with_context(|| "Invalid UTF-8 in virsh uri output")?;
        get_default_pool_path(uri_str.trim())
    };
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
    let mut cmd = virsh_command(connect_uri)?;
    cmd.args(&["pool-define", xml_path]);
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
    let mut cmd = virsh_command(connect_uri)?;
    cmd.args(&["pool-build", "default"]);
    let _ = cmd.output(); // Directory might already exist

    // Start the pool
    let mut cmd = virsh_command(connect_uri)?;
    cmd.args(&["pool-start", "default"]);
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
    let mut cmd = virsh_command(connect_uri)?;
    cmd.args(&["pool-autostart", "default"]);
    let _ = cmd.output(); // Not critical if this fails

    // Clean up temporary XML file
    let _ = std::fs::remove_file(xml_path);

    info!("Default storage pool created successfully");
    Ok(())
}

/// Get the path of the default libvirt storage pool
pub fn get_libvirt_storage_pool_path(connect_uri: Option<&str>) -> Result<Utf8PathBuf> {
    // Ensure pool exists before querying
    ensure_default_pool(connect_uri)?;

    let mut cmd = virsh_command(connect_uri)?;
    cmd.args(&["pool-dumpxml", "default"]);
    let output = cmd
        .output()
        .with_context(|| "Failed to query libvirt storage pool")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let uri_desc = connect_uri.unwrap_or("default connection");
        return Err(color_eyre::eyre::eyre!(
            "Failed to get default storage pool info for {}: {}",
            uri_desc,
            stderr
        ));
    }

    let xml = String::from_utf8(output.stdout).with_context(|| "Invalid UTF-8 in virsh output")?;
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
pub fn list_storage_pool_volumes(connect_uri: Option<&str>) -> Result<Vec<Utf8PathBuf>> {
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

/// Parse a volume mount string in the format "host_path:tag"
fn parse_volume_mount(volume_str: &str) -> Result<(String, String)> {
    let (host_part, tag_part) = volume_str.split_once(':').ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "Invalid volume format '{}'. Expected format: host_path:tag",
            volume_str
        )
    })?;

    let host_path = host_part.trim();
    let tag = tag_part.trim();

    if host_path.is_empty() || tag.is_empty() {
        return Err(color_eyre::eyre::eyre!(
            "Invalid volume format '{}'. Both host path and tag must be non-empty",
            volume_str
        ));
    }

    // Validate that the host path exists
    let host_path_buf = std::path::Path::new(host_path);
    if !host_path_buf.exists() {
        return Err(color_eyre::eyre::eyre!(
            "Host path '{}' does not exist",
            host_path
        ));
    }

    if !host_path_buf.is_dir() {
        return Err(color_eyre::eyre::eyre!(
            "Host path '{}' is not a directory",
            host_path
        ));
    }

    Ok((host_path.to_string(), tag.to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_volume_mount_valid() {
        let result = parse_volume_mount("/tmp:mytag");
        assert!(result.is_ok());
        let (host, tag) = result.unwrap();
        assert_eq!(host, "/tmp");
        assert_eq!(tag, "mytag");
    }

    #[test]
    fn test_parse_volume_mount_invalid_format() {
        let result = parse_volume_mount("/tmp");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Expected format: host_path:tag"));
    }

    #[test]
    fn test_parse_volume_mount_empty_parts() {
        let result = parse_volume_mount(":mytag");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Both host path and tag must be non-empty"));
    }

    #[test]
    fn test_parse_volume_mount_nonexistent_host() {
        let result = parse_volume_mount("/nonexistent/path/that/does/not/exist:mytag");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_parse_port_mapping_valid() {
        let result = "8080:80".parse::<PortMapping>();
        assert!(result.is_ok());
        let mapping = result.unwrap();
        assert_eq!(mapping.host_port, 8080);
        assert_eq!(mapping.guest_port, 80);
    }

    #[test]
    fn test_parse_port_mapping_same_port() {
        let result = "80:80".parse::<PortMapping>();
        assert!(result.is_ok());
        let mapping = result.unwrap();
        assert_eq!(mapping.host_port, 80);
        assert_eq!(mapping.guest_port, 80);
    }

    #[test]
    fn test_parse_port_mapping_invalid_format() {
        let result = "8080".parse::<PortMapping>();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Expected format: host_port:guest_port"));
    }

    #[test]
    fn test_parse_port_mapping_invalid_host_port() {
        let result = "abc:80".parse::<PortMapping>();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid host port"));
    }

    #[test]
    fn test_parse_port_mapping_invalid_guest_port() {
        let result = "8080:xyz".parse::<PortMapping>();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid guest port"));
    }

    #[test]
    fn test_parse_port_mapping_port_out_of_range() {
        let result = "70000:80".parse::<PortMapping>();
        assert!(result.is_err());
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
        .with_transient_disk(opts.transient)
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

    // Add labels if specified
    if !opts.label.is_empty() {
        let labels = opts.label.join(",");
        domain_builder = domain_builder.with_metadata("bootc:label", &labels);
    }

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

    // Add user-specified volume mounts
    if !opts.volumes.is_empty() {
        debug!("Processing {} volume mount(s)", opts.volumes.len());

        for volume_str in opts.volumes.iter() {
            let (host_path, tag) = parse_volume_mount(volume_str)
                .with_context(|| format!("Failed to parse volume mount '{}'", volume_str))?;

            debug!(
                "Adding volume mount: {} (host) with tag '{}'",
                host_path, tag
            );

            let virtiofs_fs = VirtiofsFilesystem {
                source_dir: host_path.clone(),
                tag: tag.clone(),
                readonly: false,
            };

            domain_builder = domain_builder.with_virtiofs_filesystem(virtiofs_fs);
        }
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

    // Build QEMU args with port forwarding
    let mut qemu_args = vec![
        "-smbios".to_string(),
        format!("type=11,value={}", smbios_cred),
    ];

    // Build netdev user mode networking with port forwarding
    let mut hostfwd_args = vec![format!("tcp::{}-:22", ssh_port)];

    // Add user-specified port mappings
    for mapping in opts.port_mappings.iter() {
        hostfwd_args.push(format!(
            "tcp::{}-:{}",
            mapping.host_port, mapping.guest_port
        ));
    }

    let netdev_config = format!(
        "user,id=ssh0,{}",
        hostfwd_args
            .iter()
            .map(|fwd| format!("hostfwd={}", fwd))
            .collect::<Vec<_>>()
            .join(",")
    );

    qemu_args.push("-netdev".to_string());
    qemu_args.push(netdev_config);
    qemu_args.push("-device".to_string());
    qemu_args.push("virtio-net-pci,netdev=ssh0,addr=0x3".to_string());

    let domain_xml = domain_builder
        .with_qemu_args(qemu_args)
        .build_xml()
        .with_context(|| "Failed to build domain XML")?;

    // Write XML to temporary file
    let xml_path = format!("/tmp/{}.xml", domain_name);
    std::fs::write(&xml_path, domain_xml).with_context(|| "Failed to write domain XML")?;

    let connect_uri = global_opts.connect.as_deref();

    // Create domain (transient or persistent)
    if opts.transient {
        // Create transient domain (single command - domain disappears on shutdown)
        run_virsh_cmd(
            connect_uri,
            &["create", &xml_path],
            "Failed to create transient libvirt domain",
        )?;
    } else {
        // Define and start the domain (persistent)
        run_virsh_cmd(
            connect_uri,
            &["define", &xml_path],
            "Failed to define libvirt domain",
        )?;
        run_virsh_cmd(
            connect_uri,
            &["start", domain_name],
            "Failed to start libvirt domain",
        )?;
    }

    // Clean up temporary XML file
    let _ = std::fs::remove_file(&xml_path);

    Ok(())
}
