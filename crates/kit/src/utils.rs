use camino::{Utf8Path, Utf8PathBuf};
use std::io::{Seek as _, Write as _};
use std::os::fd::OwnedFd;

use cap_std_ext::cap_std::io_lifetimes::AsFilelike as _;
use color_eyre::eyre::{eyre, Context};
use color_eyre::Result;

/// Creates a sealed memory file descriptor for secure data transfer.
/// The sealed memfd cannot be modified after creation, providing tamper protection.
#[allow(dead_code)]
pub(crate) fn impl_sealed_memfd(description: &str, content: &[u8]) -> Result<OwnedFd> {
    use rustix::fs::{MemfdFlags, SealFlags};
    let mfd =
        rustix::fs::memfd_create(description, MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING)?;

    {
        let mfd_file = mfd.as_filelike_view::<std::fs::File>();
        mfd_file.set_len(content.len() as u64)?;
        (&*mfd_file).write_all(content)?;
        (&*mfd_file).seek(std::io::SeekFrom::Start(0))?;
    }

    rustix::fs::fcntl_add_seals(
        &mfd,
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
    )?;
    Ok(mfd)
}

/// Detect the container storage path using podman system info
pub(crate) fn detect_container_storage_path() -> Result<Utf8PathBuf> {
    use std::process::Command;

    let output = Command::new("podman")
        .args(["system", "info", "--format", "json"])
        .output()
        .context(
            "Failed to run 'podman system info'. Ensure podman is installed and accessible.",
        )?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!("podman system info failed: {}", stderr));
    }

    let info: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("Failed to parse podman system info JSON")?;

    // Extract the graph root path from the store configuration
    let graph_root = info
        .get("store")
        .and_then(|store| store.get("graphRoot"))
        .and_then(|root| root.as_str())
        .ok_or_else(|| eyre!("Could not find graphRoot in podman system info"))?;

    let storage_path = Utf8PathBuf::from(graph_root);

    // Validate the path exists and is a directory
    if !storage_path.exists() {
        return Err(eyre!(
            "Storage path from podman does not exist: {}",
            storage_path
        ));
    }

    if !storage_path.is_dir() {
        return Err(eyre!(
            "Storage path from podman is not a directory: {}",
            storage_path
        ));
    }

    Ok(storage_path)
}

/// Validate that a container storage path exists and has the expected structure
pub(crate) fn validate_container_storage_path(path: &Utf8Path) -> Result<()> {
    if !path.exists() {
        return Err(eyre!("Container storage path does not exist: {}", path));
    }

    if !path.is_dir() {
        return Err(eyre!("Container storage path is not a directory: {}", path));
    }

    // Check for expected subdirectories that indicate this is a containers storage directory
    let overlay_path = path.join("overlay");
    let overlay_images_path = path.join("overlay-images");

    if !overlay_path.exists() && !overlay_images_path.exists() {
        return Err(eyre!(
            "Path does not appear to be a valid container storage directory: {}. Missing overlay subdirectories.", 
            path
        ));
    }

    Ok(())
}

/// Parse size string (e.g., "10G", "5120M", "1T") to bytes
pub(crate) fn parse_size(size_str: &str) -> Result<u64> {
    let size_str = size_str.trim().to_uppercase();

    if size_str.is_empty() {
        return Err(eyre!("Empty size string"));
    }

    // Try to strip known unit suffixes
    let (number_str, multiplier) = if let Some(num) = size_str.strip_suffix("TB") {
        (num, 1024_u64.pow(4))
    } else if let Some(num) = size_str.strip_suffix("GB") {
        (num, 1024 * 1024 * 1024)
    } else if let Some(num) = size_str.strip_suffix("MB") {
        (num, 1024 * 1024)
    } else if let Some(num) = size_str.strip_suffix("KB") {
        (num, 1024)
    } else if let Some(num) = size_str.strip_suffix('T') {
        (num, 1024_u64.pow(4))
    } else if let Some(num) = size_str.strip_suffix('G') {
        (num, 1024 * 1024 * 1024)
    } else if let Some(num) = size_str.strip_suffix('M') {
        (num, 1024 * 1024)
    } else if let Some(num) = size_str.strip_suffix('K') {
        (num, 1024)
    } else if let Some(num) = size_str.strip_suffix('B') {
        (num, 1)
    } else {
        // No unit suffix, assume bytes
        (&*size_str, 1)
    };

    let number: u64 = number_str
        .parse()
        .map_err(|_| eyre!("Invalid number in size: {}", number_str))?;

    Ok(number * multiplier)
}

/// Parse a memory string (like "2G", "1024M", "512") to megabytes
pub(crate) fn parse_memory_to_mb(memory_str: &str) -> Result<u32> {
    let memory_str = memory_str.trim();

    if memory_str.is_empty() {
        return Err(eyre!("Memory string cannot be empty"));
    }

    // Try to strip unit suffix, checking case-insensitively
    let (number_str, unit) = if let Some(num) = memory_str
        .strip_suffix('G')
        .or_else(|| memory_str.strip_suffix('g'))
    {
        (num, "GiB")
    } else if let Some(num) = memory_str
        .strip_suffix('M')
        .or_else(|| memory_str.strip_suffix('m'))
    {
        (num, "MiB")
    } else if let Some(num) = memory_str
        .strip_suffix('K')
        .or_else(|| memory_str.strip_suffix('k'))
    {
        (num, "KiB")
    } else {
        // No suffix, assume megabytes
        (memory_str, "MiB")
    };

    let number: f64 = number_str
        .parse()
        .context("Invalid number in memory specification")?;

    // Use libvirt helper to get bytes per unit
    let bytes_per_unit =
        crate::libvirt::unit_to_bytes(unit).ok_or_else(|| eyre!("Unknown unit: {}", unit))? as f64;

    let mib = 1024.0 * 1024.0;
    let total_mb = (number * bytes_per_unit) / mib;

    Ok(total_mb as u32)
}
