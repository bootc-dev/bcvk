//! Base disk management for libvirt VMs
//!
//! This module manages base disk images that serve as CoW sources for VM disks.
//! Base disks are cached by their DiskImageMetadata hash (image digest + install options).
//! Each VM gets a disk with a backing file using `virsh vol-create-as --backing-vol` for efficient CoW storage.

use crate::cache_metadata::DiskImageMetadata;
use crate::install_options::InstallOptions;
use camino::{Utf8Path, Utf8PathBuf};
use cap_std_ext::cap_std;
use cap_std_ext::cap_std::fs::Dir;
use cap_std_ext::dirext::CapStdExtDirExt;
use color_eyre::eyre::{eyre, Context};
use color_eyre::Result;
use tracing::{debug, info};

/// Find or create a base disk for the given parameters
pub fn find_or_create_base_disk(
    source_image: &str,
    image_digest: &str,
    install_options: &InstallOptions,
    connect_uri: Option<&str>,
) -> Result<Utf8PathBuf> {
    let metadata = DiskImageMetadata::from(install_options, image_digest);
    let cache_hash = metadata.compute_cache_hash();

    // Extract short hash for filename (first 16 chars after "sha256:")
    let short_hash = cache_hash
        .strip_prefix("sha256:")
        .unwrap_or(&cache_hash)
        .chars()
        .take(16)
        .collect::<String>();

    let base_disk_name = format!("bootc-base-{}.qcow2", short_hash);

    // Get storage pool path
    let rootfs = Dir::open_ambient_dir("/", cap_std::ambient_authority())?;
    let pool_path = super::run::get_libvirt_storage_pool_path(connect_uri)?;
    let pool_path = pool_path.strip_prefix("/").unwrap_or(&pool_path);
    let base_disk_path = pool_path.join(&base_disk_name);

    // Check if base disk already exists with valid metadata
    if let Some(pool_dir) = rootfs.open_dir_optional(pool_path)? {
        if pool_dir.try_exists(&base_disk_name)? {
            debug!("Checking existing base disk: {:?}", base_disk_path);

            if crate::cache_metadata::check_cached_disk(
                &pool_dir,
                &base_disk_name,
                image_digest,
                install_options,
            )?
            .is_ok()
            {
                return Ok(base_disk_path);
            } else {
                info!("Base disk exists but metadata doesn't match, will recreate");
                pool_dir.remove_file(&base_disk_name).with_context(|| {
                    format!("Failed to remove stale base disk: {:?}", base_disk_path)
                })?;
            }
        }

        // Base disk doesn't exist or was stale, create it
        // Multiple concurrent processes may race to create this, but each uses
        // a unique temp file, so they won't conflict
        info!("Creating base disk: {:?}", base_disk_path);
        create_base_disk(
            &pool_dir,
            Utf8Path::new(&base_disk_name),
            source_image,
            image_digest,
            install_options,
            connect_uri,
        )?;

        Ok(base_disk_path)
    } else {
        Err(eyre!("Pool directory does not exist: {}", pool_path))
    }
}

/// Create a new base disk
fn create_base_disk(
    dir: &Dir,
    base_disk_name: &Utf8Path,
    source_image: &str,
    image_digest: &str,
    install_options: &InstallOptions,
    connect_uri: Option<&str>,
) -> Result<()> {
    use crate::run_ephemeral::CommonVmOpts;
    use crate::to_disk::{Format, ToDiskAdditionalOpts, ToDiskOpts};

    // Use a unique temporary file to avoid conflicts when multiple processes
    // race to create the same base disk
    // We use a temp name pattern that includes a unique component to avoid collisions
    let temp_name = format!(".{}.{}.tmp", base_disk_name, std::process::id());

    // Get the actual path to use (need absolute path for to_disk)
    let pool_path = super::run::get_libvirt_storage_pool_path(connect_uri)?;
    let temp_disk_path = pool_path.join(&temp_name);

    // Create a guard that will clean up the temp file on drop
    struct TempFileGuard<'a> {
        dir: &'a Dir,
        name: String,
    }

    impl Drop for TempFileGuard<'_> {
        fn drop(&mut self) {
            let _ = self.dir.remove_file(&self.name);
        }
    }

    let _temp_guard = TempFileGuard {
        dir,
        name: temp_name.clone(),
    };

    // Keep the temp file open so it gets cleaned up automatically if we error out
    // We'll persist it manually on success

    // Create the disk using to_disk at temporary location
    let to_disk_opts = ToDiskOpts {
        source_image: source_image.to_string(),
        target_disk: temp_disk_path.clone(),
        install: install_options.clone(),
        additional: ToDiskAdditionalOpts {
            disk_size: install_options
                .root_size
                .clone()
                .or(Some(super::LIBVIRT_DEFAULT_DISK_SIZE.to_string())),
            format: Format::Qcow2, // Use qcow2 for CoW cloning
            common: CommonVmOpts {
                memory: crate::common_opts::MemoryOpts {
                    memory: super::LIBVIRT_DEFAULT_MEMORY.to_string(),
                },
                ..Default::default()
            },
            ..Default::default()
        },
    };

    // Run bootc install - if it succeeds, the disk is valid
    // On error, temp_guard is automatically cleaned up when dropped
    crate::to_disk::run(to_disk_opts)
        .with_context(|| format!("Failed to install bootc to base disk: {:?}", temp_disk_path))?;

    // If we got here, bootc install succeeded - verify metadata was written
    // Verify by reading back the xattrs using cap-std
    match crate::cache_metadata::check_cached_disk(dir, &temp_name, image_digest, install_options)?
    {
        Ok(()) => {}
        Err(e) => {
            return Err(eyre!("Generated disk metadata validation failed: {e}"));
        }
    }

    // All validations passed - rename temp file to final location
    // If another concurrent process already created the file, that's fine
    match dir.rename(&temp_name, dir, base_disk_name) {
        Ok(_) => {
            debug!("Successfully created base disk: {:?}", base_disk_name);
            // Don't clean up the temp file since we successfully renamed it
            std::mem::forget(_temp_guard);
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Another process won the race and created the base disk
            debug!(
                "Base disk already created by another process: {:?}",
                base_disk_name
            );
            // temp file is cleaned up when _temp_guard is dropped
        }
        Err(e) => {
            return Err(e)
                .with_context(|| format!("Failed to persist base disk to {:?}", base_disk_name));
        }
    }

    // Refresh libvirt storage pool so the new disk is visible to virsh
    let mut cmd = super::run::virsh_command(connect_uri)?;
    cmd.args(&["pool-refresh", "default"]);

    if let Err(e) = cmd
        .output()
        .with_context(|| "Failed to run virsh pool-refresh")
    {
        debug!("Warning: Failed to refresh libvirt storage pool: {}", e);
        // Don't fail if pool refresh fails, the disk was created successfully
    }

    info!(
        "Successfully created and validated base disk: {:?}",
        base_disk_name
    );
    Ok(())
}

/// Clone a base disk to create a VM-specific disk
///
/// Uses predictable disk name: `{vm_name}.qcow2`
/// If the disk already exists, it will be deleted using `virsh vol-delete` first.
pub fn clone_from_base(
    base_disk_path: &Utf8Path,
    vm_name: &str,
    connect_uri: Option<&str>,
) -> Result<Utf8PathBuf> {
    let rootfs = Dir::open_ambient_dir("/", cap_std::ambient_authority())?;
    let pool_path = super::run::get_libvirt_storage_pool_path(connect_uri)?;
    let pool_path = pool_path.strip_prefix("/").unwrap_or(&pool_path);
    let pool_dir = rootfs
        .open_dir(pool_path)
        .with_context(|| format!("Failed to open pool directory {:?}", pool_path))?;

    // Use predictable disk name
    let vm_disk_name = format!("{}.qcow2", vm_name);
    let vm_disk_path = pool_path.join(&vm_disk_name);

    // Refresh the storage pool so libvirt knows about all files
    let mut refresh_cmd = super::run::virsh_command(connect_uri)?;
    refresh_cmd.args(&["pool-refresh", "default"]);
    let _ = refresh_cmd.output(); // Ignore errors, pool might not exist yet

    // Try to delete the volume if it exists (either as a file or in libvirt's view)
    // This handles both cases: file exists but not tracked, or tracked by libvirt
    let mut cmd = super::run::virsh_command(connect_uri)?;
    cmd.args(&["vol-delete", "--pool", "default", &vm_disk_name]);

    let output = cmd
        .output()
        .with_context(|| "Failed to run virsh vol-delete")?;

    if output.status.success() {
        info!("Deleted existing disk volume: {}", vm_disk_name);
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // If volume doesn't exist, that's fine - we'll create it
        // Only error if it exists but we can't delete it (e.g., in use)
        if !stderr.contains("Storage volume not found") && !stderr.contains("no storage vol") {
            return Err(color_eyre::eyre::eyre!(
                "Failed to delete existing volume '{}': {}",
                vm_disk_name,
                stderr
            ));
        }
        debug!(
            "Volume {} doesn't exist in pool, will create it",
            vm_disk_name
        );
    }

    // Also remove the file if it exists but wasn't tracked by libvirt
    if pool_dir.try_exists(&vm_disk_name)? {
        debug!("Removing untracked disk file: {:?}", vm_disk_path);
        pool_dir
            .remove_file(&vm_disk_name)
            .with_context(|| format!("Failed to remove disk file: {:?}", vm_disk_path))?;
    }

    debug!(
        "Creating VM disk with backing file: {:?} -> {:?}",
        base_disk_path, vm_disk_path
    );

    // Get the virtual size of the base disk to use for the new volume
    let info = crate::qemu_img::info(&pool_dir, base_disk_path)?;
    let virtual_size = info.virtual_size;

    // Create volume with backing file using vol-create-as
    // This creates a qcow2 image with the base disk as backing file (proper CoW)
    let base_disk_filename = base_disk_path.file_name().ok_or_else(|| {
        color_eyre::eyre::eyre!("Base disk path has no filename: {:?}", base_disk_path)
    })?;

    let mut cmd = super::run::virsh_command(connect_uri)?;
    cmd.args(&[
        "vol-create-as",
        "default",
        &vm_disk_name,
        &virtual_size.to_string(),
        "--format",
        "qcow2",
        "--backing-vol",
        base_disk_filename,
        "--backing-vol-format",
        "qcow2",
    ]);

    let output = cmd
        .output()
        .with_context(|| "Failed to run virsh vol-create-as")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(color_eyre::eyre::eyre!(
            "Failed to create VM disk with backing file: {}",
            stderr
        ));
    }

    debug!(
        "Successfully created VM disk with backing file: {:?}",
        vm_disk_path
    );
    Ok(vm_disk_path)
}

/// List all base disks in the storage pool with reference counts
pub fn list_base_disks(connect_uri: Option<&str>) -> Result<Vec<BaseDiskInfo>> {
    use super::run::list_storage_pool_volumes;

    let rootfs = Dir::open_ambient_dir("/", cap_std::ambient_authority())?;

    let pool_path = super::run::get_libvirt_storage_pool_path(connect_uri)?;
    let mut base_disks = Vec::new();

    // Get all volumes to count references
    let all_volumes = list_storage_pool_volumes(connect_uri)?;
    let vm_disks: Vec<_> = all_volumes
        .iter()
        .filter(|p| {
            if let Some(name) = p.file_name() {
                !name.starts_with("bootc-base-")
            } else {
                false
            }
        })
        .collect();

    if let Some(d) = rootfs.open_dir_optional(&pool_path)? {
        for entry in d.entries()? {
            let entry = entry?;
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str().map(Utf8Path::new) else {
                continue;
            };
            let path = pool_path.join(name);
            let Some("qcow2") = name.extension() else {
                continue;
            };
            if !name.starts_with("bootc-base-") {
                continue;
            }

            // Try to read metadata
            let image_digest =
                crate::cache_metadata::DiskImageMetadata::read_image_digest_from_path(
                    &d, &file_name,
                )
                .unwrap_or(None);

            // Get file size and creation time
            let metadata = entry.metadata()?;
            let size = Some(metadata.len());
            let created = metadata.created()?.into_std();

            // Count references
            let ref_count = count_base_disk_references(&d, &name, &vm_disks)?;

            base_disks.push(BaseDiskInfo {
                path,
                image_digest,
                size,
                ref_count,
                created,
            });
        }
    }

    Ok(base_disks)
}

/// Information about a base disk
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct BaseDiskInfo {
    pub path: Utf8PathBuf,
    pub image_digest: Option<String>,
    pub size: Option<u64>,
    pub ref_count: usize,
    pub created: std::time::SystemTime,
}

/// Prune unreferenced base disks
pub fn prune_base_disks(connect_uri: Option<&str>, dry_run: bool) -> Result<Vec<Utf8PathBuf>> {
    use super::run::list_storage_pool_volumes;

    let rootfs = Dir::open_ambient_dir("/", cap_std::ambient_authority())?;
    let pool_path = super::run::get_libvirt_storage_pool_path(connect_uri)?;
    let pool_dir = rootfs
        .open_dir(pool_path.strip_prefix("/").unwrap_or(&pool_path))
        .with_context(|| format!("Failed to open pool directory {:?}", pool_path))?;

    let base_disks = list_base_disks(connect_uri)?;
    let all_volumes = list_storage_pool_volumes(connect_uri)?;

    // Collect all non-base volumes (VM disks)
    let vm_disks: Vec<_> = all_volumes
        .iter()
        .filter(|p| {
            if let Some(name) = p.file_name() {
                !name.starts_with("bootc-base-")
            } else {
                false
            }
        })
        .collect();

    let mut pruned = Vec::new();

    for base_disk in base_disks {
        // Check if any VM disk references this base
        let is_referenced = check_base_disk_referenced(&pool_dir, &base_disk.path, &vm_disks)?;

        if !is_referenced {
            info!("Base disk not referenced by any VM: {:?}", base_disk.path);

            if dry_run {
                println!("Would remove: {}", base_disk.path);
            } else {
                // Use virsh vol-delete to properly unregister from libvirt storage pool
                let base_disk_name = base_disk.path.file_name().ok_or_else(|| {
                    color_eyre::eyre::eyre!("Base disk path has no filename: {:?}", base_disk.path)
                })?;

                let mut cmd = super::run::virsh_command(connect_uri)?;
                cmd.args(&["vol-delete", "--pool", "default", base_disk_name]);

                let output = cmd.output().with_context(|| {
                    format!("Failed to run virsh vol-delete for {}", base_disk_name)
                })?;

                if !output.status.success() {
                    let stderr = String::from_utf8(output.stderr)
                        .with_context(|| "Invalid UTF-8 in virsh stderr")?;
                    return Err(color_eyre::eyre::eyre!(
                        "Failed to delete base disk volume '{}': {}",
                        base_disk_name,
                        stderr
                    ));
                }
                println!("Removed: {}", base_disk.path);
            }

            pruned.push(base_disk.path);
        }
    }

    Ok(pruned)
}

/// Count how many VM disks reference a specific base disk
fn count_base_disk_references(
    dir: &Dir,
    base_disk: impl AsRef<Utf8Path>,
    vm_disks: &[&Utf8PathBuf],
) -> Result<usize> {
    let base_disk = base_disk.as_ref();
    let base_disk_name = base_disk
        .file_name()
        .ok_or_else(|| eyre!("Missing filename {base_disk}"))?;
    let mut count = 0;

    for vm_disk in vm_disks {
        // Use qemu-img info with --force-share to allow reading even if disk is locked by a running VM
        let info = match crate::qemu_img::info(dir, vm_disk) {
            Ok(info) => info,
            Err(_) => {
                // If we can't read the disk, skip it for counting purposes
                // (We're conservative in check_base_disk_referenced but here we just want a count)
                debug!(
                    "Warning: Could not read disk info for {:?}, skipping for reference count",
                    vm_disk
                );
                continue;
            }
        };

        // Check both "backing-filename" and "full-backing-filename" fields
        if let Some(backing_file) = &info.backing_filename {
            if backing_file.contains(base_disk_name) {
                count += 1;
                continue;
            }
        }
        if let Some(backing_file) = &info.full_backing_filename {
            if backing_file.contains(base_disk_name) {
                count += 1;
            }
        }
    }

    Ok(count)
}

/// Check if a base disk is referenced by any VM disk (via qcow2 backing file)
fn check_base_disk_referenced(
    dir: &Dir,
    base_disk: &Utf8Path,
    vm_disks: &[&Utf8PathBuf],
) -> Result<bool> {
    let base_disk_name = base_disk.file_name().unwrap();

    for vm_disk in vm_disks {
        // Use qemu-img info with --force-share to allow reading even if disk is locked by a running VM
        let info = match crate::qemu_img::info(dir, vm_disk) {
            Ok(info) => info,
            Err(e) => {
                // If we can't read the disk info, be conservative and assume it DOES reference this base
                // This prevents accidentally pruning base disks that are in use
                debug!(
                    "Warning: Could not read disk info for {:?}, conservatively assuming it references base disk: {}",
                    vm_disk, e
                );
                return Ok(true);
            }
        };

        // Check both "backing-filename" and "full-backing-filename" fields
        if let Some(backing_file) = &info.backing_filename {
            if backing_file.contains(base_disk_name) {
                debug!(
                    "Found backing file reference: {:?} -> {:?}",
                    vm_disk, backing_file
                );
                return Ok(true);
            }
        }
        if let Some(backing_file) = &info.full_backing_filename {
            if backing_file.contains(base_disk_name) {
                debug!(
                    "Found full backing file reference: {:?} -> {:?}",
                    vm_disk, backing_file
                );
                return Ok(true);
            }
        }
    }

    Ok(false)
}
