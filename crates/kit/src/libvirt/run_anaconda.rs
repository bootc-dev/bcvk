//! libvirt run-anaconda command - run a bootable container as a VM installed via anaconda
//!
//! This module provides functionality for creating and managing libvirt-based VMs
//! from bootc container images using anaconda for installation. Unlike `libvirt run`
//! which uses `bootc install to-disk`, this command uses anaconda with kickstart files
//! for more flexible partitioning and system configuration.
//!
//! This module shares most of its implementation with `libvirt run`, only differing
//! in the base disk creation phase which uses anaconda instead of `bootc install to-disk`.

use camino::Utf8PathBuf;
use clap::Parser;
use color_eyre::eyre::{eyre, Context};
use color_eyre::Result;
use tracing::{debug, info};

use super::run::{BindMount, FirmwareType, LibvirtRunOpts, PortMapping};
use crate::common_opts::MemoryOpts;
use crate::install_options::InstallOptions;

/// Default anaconda installer image
const DEFAULT_ANACONDA_IMAGE: &str = "localhost/anaconda-bootc:latest";

/// Options for creating and running a bootable container VM via anaconda
///
/// This struct mirrors `LibvirtRunOpts` but adds anaconda-specific options
/// (kickstart, target_imgref, anaconda_image). The VM lifecycle (SSH, networking,
/// bind mounts, etc.) is identical to `libvirt run`.
#[derive(Debug, Parser)]
pub struct LibvirtRunAnacondaOpts {
    /// Container image to run as a bootable VM
    pub image: String,

    /// Kickstart file with partitioning and system configuration
    ///
    /// Must contain partitioning (e.g., autopart), locale settings (lang,
    /// keyboard, timezone), and other system configuration. The `ostreecontainer`
    /// directive, and `%post` registry repointing are injected automatically.
    #[clap(long, short = 'k')]
    pub kickstart: std::path::PathBuf,

    /// Name for the VM (auto-generated if not specified)
    #[clap(long)]
    pub name: Option<String>,

    /// Replace existing VM with same name (stop and remove if exists)
    #[clap(long, short = 'R')]
    pub replace: bool,

    /// Target image reference for the installed system
    ///
    /// After installation, the system's bootc origin is repointed to this
    /// registry image so that `bootc upgrade` pulls updates from the registry
    /// rather than expecting containers-storage. Defaults to the image argument.
    #[clap(long)]
    pub target_imgref: Option<String>,

    /// Skip injecting the %post script that repoints to target-imgref
    ///
    /// Use this if you want to handle bootc origin configuration yourself
    /// in your kickstart file.
    #[clap(long)]
    pub no_repoint: bool,

    /// Anaconda container image to use as the installer
    #[clap(long, default_value = DEFAULT_ANACONDA_IMAGE)]
    pub anaconda_image: String,

    #[clap(
        long,
        help = "Instance type (e.g., u1.nano, u1.small, u1.medium). Overrides cpus/memory if specified."
    )]
    pub itype: Option<crate::instancetypes::InstanceType>,

    #[clap(flatten)]
    pub memory: MemoryOpts,

    /// Number of virtual CPUs for the VM (overridden by --itype if specified)
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

    /// Volume mount from host to VM (raw virtiofs tag, for manual mounting)
    #[clap(long = "volume", short = 'v', action = clap::ArgAction::Append)]
    pub raw_volumes: Vec<String>,

    /// Bind mount from host to VM (format: host_path:guest_path)
    #[clap(long = "bind", action = clap::ArgAction::Append)]
    pub bind_mounts: Vec<BindMount>,

    /// Bind mount from host to VM as read-only (format: host_path:guest_path)
    #[clap(long = "bind-ro", action = clap::ArgAction::Append)]
    pub bind_mounts_ro: Vec<BindMount>,

    /// Network mode for the VM
    #[clap(long, default_value = "user")]
    pub network: String,

    /// Keep the VM running in background after creation
    #[clap(long)]
    pub detach: bool,

    /// Automatically SSH into the VM after creation
    #[clap(long)]
    pub ssh: bool,

    /// Wait for SSH to become available and verify connectivity (for testing)
    #[clap(long, conflicts_with = "ssh")]
    pub ssh_wait: bool,

    /// Mount host container storage (RO) at /run/host-container-storage
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

impl LibvirtRunAnacondaOpts {
    /// Validate that labels don't contain commas
    fn validate_labels(&self) -> Result<()> {
        super::run::validate_labels(&self.label)
    }

    /// Convert to LibvirtRunOpts for domain creation (reuses all the domain creation logic)
    fn to_libvirt_run_opts(&self) -> LibvirtRunOpts {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("bootc:install-method".to_string(), "anaconda".to_string());

        LibvirtRunOpts {
            image: self.image.clone(),
            name: self.name.clone(),
            replace: self.replace,
            itype: self.itype,
            memory: self.memory.clone(),
            cpus: self.cpus,
            disk_size: self.disk_size.clone(),
            install: self.install.clone(),
            port_mappings: self.port_mappings.clone(),
            raw_volumes: self.raw_volumes.clone(),
            bind_mounts: self.bind_mounts.clone(),
            bind_mounts_ro: self.bind_mounts_ro.clone(),
            network: self.network.clone(),
            detach: self.detach,
            ssh: self.ssh,
            ssh_wait: self.ssh_wait,
            bind_storage_ro: self.bind_storage_ro,
            update_from_host: false, // anaconda doesn't use this
            firmware: self.firmware,
            disable_tpm: self.disable_tpm,
            secure_boot_keys: self.secure_boot_keys.clone(),
            label: self.label.clone(),
            transient: self.transient,
            metadata,
            extra_smbios_credentials: Vec::new(),
        }
    }
}

/// Execute the libvirt run-anaconda command
pub fn run(
    global_opts: &crate::libvirt::LibvirtOptions,
    opts: LibvirtRunAnacondaOpts,
) -> Result<()> {
    use crate::domain_list::DomainLister;
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

    // Generate or validate VM name (reuse shared function)
    let vm_name = match &opts.name {
        Some(name) => {
            if existing_domains.contains(name) {
                if opts.replace {
                    println!("Replacing existing VM '{}'...", name);
                    crate::libvirt::rm::remove_vm_forced(global_opts, name, true)
                        .with_context(|| format!("Failed to remove existing VM '{}'", name))?;
                } else {
                    return Err(eyre!(
                        "VM '{}' already exists. Use --replace to replace it.",
                        name
                    ));
                }
            }
            name.clone()
        }
        None => super::run::generate_unique_vm_name(&opts.image, &existing_domains),
    };

    println!(
        "Creating libvirt domain '{}' via anaconda (install source: {})",
        vm_name, opts.image
    );

    // Get the image digest for caching
    let inspect = images::inspect(&opts.image)?;
    let image_digest = inspect.digest.to_string();
    debug!("Image digest: {}", image_digest);

    // Phase 1: Find or create a base disk using anaconda
    let base_disk_path = find_or_create_anaconda_base_disk(
        &opts.image,
        &image_digest,
        &opts.kickstart,
        opts.target_imgref.as_deref(),
        opts.no_repoint,
        &opts.anaconda_image,
        &opts.install,
        connect_uri,
    )
    .with_context(|| "Failed to find or create anaconda base disk")?;

    println!("Using base disk image: {}", base_disk_path);

    // Phase 2: Clone the base disk to create a VM-specific disk (reuse shared function)
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

    // Phase 3: Create libvirt domain using shared domain creation logic
    println!("Creating libvirt domain...");

    // Convert to LibvirtRunOpts and use the shared domain creation function
    let mut run_opts = opts.to_libvirt_run_opts();
    run_opts.name = Some(vm_name.clone());

    super::run::create_libvirt_domain_from_disk(
        &vm_name,
        &disk_path,
        &image_digest,
        &run_opts,
        global_opts,
    )
    .with_context(|| "Failed to create libvirt domain")?;

    // Print success info
    let resolved_memory = run_opts.resolved_memory_mb()?;
    let resolved_cpus = run_opts.resolved_cpus()?;

    println!("VM '{}' created successfully!", vm_name);
    println!("  Image: {}", opts.image);
    println!("  Install method: anaconda");
    println!("  Disk: {}", disk_path);
    if let Some(ref itype) = opts.itype {
        println!("  Instance Type: {}", itype);
    }
    println!("  Memory: {} MiB", resolved_memory);
    println!("  CPUs: {}", resolved_cpus);

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

    // Handle SSH options (reuse shared wait function)
    if opts.ssh_wait {
        super::run::wait_for_ssh_ready(
            global_opts,
            &vm_name,
            super::run::SSH_WAIT_TIMEOUT_SECONDS,
        )?;
        println!("Ready; use bcvk libvirt ssh to connect");
        Ok(())
    } else if opts.ssh {
        super::run::wait_for_ssh_ready(
            global_opts,
            &vm_name,
            super::run::SSH_WAIT_TIMEOUT_SECONDS,
        )?;

        let ssh_opts = crate::libvirt::ssh::LibvirtSshOpts {
            domain_name: vm_name,
            user: "root".to_string(),
            command: vec![],
            suppress_output: false,
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

/// Find or create a base disk using anaconda installation
///
/// This is the only part that differs from `libvirt run` - instead of using
/// `bootc install to-disk`, we use anaconda with a kickstart file.
fn find_or_create_anaconda_base_disk(
    source_image: &str,
    image_digest: &str,
    kickstart: &std::path::Path,
    target_imgref: Option<&str>,
    no_repoint: bool,
    anaconda_image: &str,
    install_options: &InstallOptions,
    connect_uri: Option<&str>,
) -> Result<Utf8PathBuf> {
    use sha2::{Digest, Sha256};

    // Read kickstart content to include in cache hash
    let kickstart_content = std::fs::read_to_string(kickstart)
        .with_context(|| format!("Failed to read kickstart: {}", kickstart.display()))?;

    // Compute a cache hash that includes all inputs that affect the resulting disk:
    // - image digest
    // - kickstart content hash
    // - repoint setting
    // - install options (filesystem, root-size, composefs, bootloader, kargs)
    let cache_hash = {
        let mut hasher = Sha256::new();
        hasher.update(image_digest.as_bytes());
        hasher.update(b"|anaconda|");
        hasher.update(kickstart_content.as_bytes());
        hasher.update(format!("|repoint:{}|", !no_repoint).as_bytes());
        if let Some(fs) = &install_options.filesystem {
            hasher.update(format!("fs:{}", fs).as_bytes());
        }
        if let Some(size) = &install_options.root_size {
            hasher.update(format!("|size:{}", size).as_bytes());
        }
        for karg in &install_options.karg {
            hasher.update(format!("|karg:{}", karg).as_bytes());
        }
        if install_options.composefs_backend {
            hasher.update(b"|composefs:true");
        }
        if let Some(ref bl) = install_options.bootloader {
            hasher.update(format!("|bootloader:{}", bl).as_bytes());
        }
        format!("sha256:{:x}", hasher.finalize())
    };

    let short_hash = cache_hash
        .strip_prefix("sha256:")
        .unwrap_or(&cache_hash)
        .chars()
        .take(16)
        .collect::<String>();

    // Use different prefix to distinguish from to-disk base disks
    let base_disk_name = format!("bootc-base-anaconda-{}.qcow2", short_hash);

    let pool_path = super::run::get_libvirt_storage_pool_path(connect_uri)?;
    let base_disk_path = pool_path.join(&base_disk_name);

    // Check if base disk already exists
    if base_disk_path.exists() {
        debug!("Found existing anaconda base disk: {:?}", base_disk_path);
        // For anaconda disks, we trust the hash-based naming since the kickstart
        // content hash is included in the filename
        return Ok(base_disk_path);
    }

    // Base disk doesn't exist, create it
    info!("Creating anaconda base disk: {:?}", base_disk_path);
    create_anaconda_base_disk(
        &base_disk_path,
        source_image,
        image_digest,
        kickstart,
        target_imgref,
        no_repoint,
        anaconda_image,
        install_options,
        connect_uri,
    )?;

    Ok(base_disk_path)
}

/// Create a new base disk using anaconda installation
fn create_anaconda_base_disk(
    base_disk_path: &camino::Utf8Path,
    source_image: &str,
    image_digest: &str,
    kickstart: &std::path::Path,
    target_imgref: Option<&str>,
    no_repoint: bool,
    anaconda_image: &str,
    install_options: &InstallOptions,
    connect_uri: Option<&str>,
) -> Result<()> {
    use crate::anaconda::install::AnacondaInstallOpts;
    use crate::run_ephemeral::CommonVmOpts;
    use crate::to_disk::Format;
    use crate::utils::DiskSize;

    // Calculate disk size
    let disk_size = install_options
        .root_size
        .as_ref()
        .and_then(|s| s.parse::<DiskSize>().ok())
        .unwrap_or_else(|| {
            super::LIBVIRT_DEFAULT_DISK_SIZE
                .parse::<DiskSize>()
                .expect("Default disk size should parse")
        });

    // Generate a unique temporary path. We can't use tempfile::NamedTempFile because
    // anaconda::install() creates its own file at the target path using qemu-img,
    // which would conflict with the tempfile handle.
    let temp_disk_name = format!(
        "{}.{}.tmp.qcow2",
        base_disk_path.file_stem().unwrap(),
        uuid::Uuid::new_v4().simple()
    );
    let temp_disk_path = base_disk_path.parent().unwrap().join(&temp_disk_name);

    // Run the installation in a closure so we can clean up the temp file on any error
    let result = (|| -> Result<()> {
        // Build anaconda install options
        let anaconda_opts = AnacondaInstallOpts {
            image: source_image.to_string(),
            target_disk: temp_disk_path.clone(),
            kickstart: kickstart.to_path_buf(),
            target_imgref: target_imgref.map(|s| s.to_string()),
            no_repoint,
            anaconda_image: anaconda_image.to_string(),
            disk_size: Some(disk_size),
            format: Format::Qcow2,
            install: install_options.clone(),
            common: CommonVmOpts {
                memory: crate::common_opts::MemoryOpts {
                    memory: super::LIBVIRT_DEFAULT_MEMORY.to_string(),
                },
                ..Default::default()
            },
        };

        // Run anaconda installation
        info!("Running anaconda installation to create base disk...");
        crate::anaconda::install::install(&crate::anaconda::AnacondaOptions {}, anaconda_opts)
            .with_context(|| "Anaconda installation failed")?;

        // Write cache metadata as xattrs
        let metadata = crate::cache_metadata::DiskImageMetadata::from(
            install_options,
            image_digest,
            source_image,
        );
        let file = std::fs::File::open(&temp_disk_path)
            .with_context(|| format!("Failed to open disk for metadata: {}", temp_disk_path))?;
        metadata
            .write_to_file(&file)
            .with_context(|| "Failed to write cache metadata to disk")?;
        drop(file); // Close file before rename

        // Atomically rename temp file to final location
        std::fs::rename(&temp_disk_path, base_disk_path)
            .with_context(|| format!("Failed to persist base disk to {:?}", base_disk_path))?;

        debug!(
            "Successfully created anaconda base disk: {:?}",
            base_disk_path
        );
        Ok(())
    })();

    // Clean up temp file on error
    if result.is_err() && temp_disk_path.exists() {
        let _ = std::fs::remove_file(&temp_disk_path);
    }

    result?;

    // Refresh libvirt storage pool so the new disk is visible
    let mut cmd = super::run::virsh_command(connect_uri)?;
    cmd.args(["pool-refresh", "default"]);
    if let Err(e) = cmd.output() {
        debug!("Warning: Failed to refresh libvirt storage pool: {}", e);
    }

    info!(
        "Successfully created anaconda base disk: {:?}",
        base_disk_path
    );
    Ok(())
}
