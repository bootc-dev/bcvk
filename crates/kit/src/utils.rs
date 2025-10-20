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

    let (number_part, unit_part) = if let Some(pos) = size_str.rfind(|c: char| c.is_ascii_digit()) {
        let (num, unit) = size_str.split_at(pos + 1);
        (num, unit)
    } else {
        return Err(eyre!("Invalid size format: {}", size_str));
    };

    let number: u64 = number_part
        .parse()
        .map_err(|_| eyre!("Invalid number in size: {}", number_part))?;

    let multiplier = match unit_part {
        "" | "B" => 1,
        "K" | "KB" => 1024,
        "M" | "MB" => 1024 * 1024,
        "G" | "GB" => 1024 * 1024 * 1024,
        "T" | "TB" => 1024_u64.pow(4),
        _ => return Err(eyre!("Unknown size unit: {}", unit_part)),
    };

    Ok(number * multiplier)
}

/// Parse a memory string (like "2G", "1024M", "512") to megabytes
pub(crate) fn parse_memory_to_mb(memory_str: &str) -> Result<u32> {
    let memory_str = memory_str.trim();

    if memory_str.is_empty() {
        return Err(eyre!("Memory string cannot be empty"));
    }

    // Check if it ends with a unit suffix
    if let Some(last_char) = memory_str.chars().last() {
        match last_char.to_ascii_uppercase() {
            'G' => {
                let number_part = &memory_str[..memory_str.len() - 1];
                let gb: f64 = number_part
                    .parse()
                    .context("Invalid number in memory specification")?;
                Ok((gb * 1024.0) as u32)
            }
            'M' => {
                let number_part = &memory_str[..memory_str.len() - 1];
                let mb: u32 = number_part
                    .parse()
                    .context("Invalid number in memory specification")?;
                Ok(mb)
            }
            'K' => {
                let number_part = &memory_str[..memory_str.len() - 1];
                let kb: u32 = number_part
                    .parse()
                    .context("Invalid number in memory specification")?;
                Ok(kb / 1024)
            }
            _ => {
                // No suffix, assume megabytes
                let mb: u32 = memory_str
                    .parse()
                    .context("Invalid number in memory specification")?;
                Ok(mb)
            }
        }
    } else {
        Err(eyre!("Memory specification cannot be empty - please provide a value like '2G', '1024M', or '512'"))
    }
}

/// Convert memory value with unit to megabytes (MiB)
/// Handles libvirt-style units distinguishing between decimal (KB, MB, GB - powers of 1000)
/// and binary (KiB, MiB, GiB - powers of 1024) units per libvirt specification
pub(crate) fn convert_memory_to_mb(value: u32, unit: &str) -> u32 {
    // Use u128 for calculations to prevent overflow with large units like TB
    let value_u128 = value as u128;
    let mib_u128 = 1024 * 1024;

    let mb = match unit {
        // Binary prefixes (powers of 1024), converting to MiB
        "k" | "K" | "KiB" => value_u128 / 1024,
        "M" | "MiB" => value_u128,
        "G" | "GiB" => value_u128 * 1024,
        "T" | "TiB" => value_u128 * 1024 * 1024,

        // Decimal prefixes (powers of 1000), converting to MiB
        "B" | "bytes" => value_u128 / mib_u128,
        "KB" => (value_u128 * 1_000) / mib_u128,
        "MB" => (value_u128 * 1_000_000) / mib_u128,
        "GB" => (value_u128 * 1_000_000_000) / mib_u128,
        "TB" => (value_u128 * 1_000_000_000_000) / mib_u128,

        // Libvirt default is KiB for memory
        _ => value_u128 / 1024,
    };
    mb as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_memory_to_mb() {
        // Test binary units (powers of 1024)
        assert_eq!(convert_memory_to_mb(4194304, "KiB"), 4096);
        assert_eq!(convert_memory_to_mb(2097152, "KiB"), 2048);
        assert_eq!(convert_memory_to_mb(2048, "MiB"), 2048);
        assert_eq!(convert_memory_to_mb(4096, "MiB"), 4096);
        assert_eq!(convert_memory_to_mb(4, "GiB"), 4096);
        assert_eq!(convert_memory_to_mb(2, "GiB"), 2048);

        // Test short forms (binary)
        assert_eq!(convert_memory_to_mb(4, "G"), 4096);
        assert_eq!(convert_memory_to_mb(2048, "M"), 2048);
        assert_eq!(convert_memory_to_mb(2097152, "K"), 2048);

        // Test decimal units (powers of 1000)
        assert_eq!(convert_memory_to_mb(1048576, "KB"), 1000);
        assert_eq!(convert_memory_to_mb(1024, "MB"), 976);
        assert_eq!(convert_memory_to_mb(4, "GB"), 3814);

        // Test default/unknown unit (defaults to KiB)
        assert_eq!(convert_memory_to_mb(4194304, "unknown"), 4096);
    }
}
