//! Cache metadata management for bootc disk images
//!
//! This module provides functionality for storing and retrieving metadata about bootc disk images
//! using extended attributes (xattrs). This enables efficient caching by allowing bcvk to detect
//! when a disk image can be reused instead of regenerating it.
//!
//! The cache system stores two separate xattrs:
//! - A SHA256 hash of all build inputs for cache validation
//! - The container image digest for visibility and tracking

use crate::install_options::InstallOptions;
use cap_std_ext::cap_std::{self, fs::Dir};
use cap_std_ext::dirext::CapStdExtDirExt;
use color_eyre::{eyre::Context, Result};
use std::ffi::OsStr;
use std::fs::File;
use std::path::Path;

/// Extended attribute name for storing bootc cache hash
const BOOTC_CACHE_HASH_XATTR: &str = "user.bootc.cache_hash";

/// Extended attribute name for storing container image digest
const BOOTC_IMAGE_DIGEST_XATTR: &str = "user.bootc.image_digest";

/// Metadata stored on disk images for caching purposes
#[derive(Debug, Clone)]
pub struct DiskImageMetadata {
    /// SHA256 digest of the source container image
    pub digest: String,
}

impl DiskImageMetadata {
    /// Generate SHA256 hash of all build inputs
    ///
    /// Delegates to InstallOptions::compute_hash() to avoid duplication.
    /// This ensures the hash includes all fields that affect the generated disk,
    /// including target_transport.
    pub fn compute_cache_hash(&self, install_options: &InstallOptions) -> String {
        install_options.compute_hash(&self.digest)
    }

    /// Write metadata to a file using extended attributes via rustix
    pub fn write_to_file(&self, file: &File, install_options: &InstallOptions) -> Result<()> {
        // Write the cache hash
        let cache_hash = self.compute_cache_hash(install_options);
        rustix::fs::fsetxattr(
            file,
            BOOTC_CACHE_HASH_XATTR,
            cache_hash.as_bytes(),
            rustix::fs::XattrFlags::empty(),
        )
        .with_context(|| "Failed to set cache hash xattr")?;

        // Write the image digest separately for visibility
        rustix::fs::fsetxattr(
            file,
            BOOTC_IMAGE_DIGEST_XATTR,
            self.digest.as_bytes(),
            rustix::fs::XattrFlags::empty(),
        )
        .with_context(|| "Failed to set image digest xattr")?;

        tracing::debug!(
            "Wrote cache hash {} and image digest {} to disk image",
            cache_hash,
            self.digest
        );
        Ok(())
    }

    /// Read image digest from a file path using extended attributes
    pub fn read_image_digest_from_path(path: &Path) -> Result<Option<String>> {
        // First check if file exists
        if !path.exists() {
            return Ok(None);
        }

        // Get the parent directory and file name
        let parent = path
            .parent()
            .ok_or_else(|| color_eyre::eyre::eyre!("Path has no parent directory"))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| color_eyre::eyre::eyre!("Path has no file name"))?;

        // Open the parent directory with cap-std
        let dir = Dir::open_ambient_dir(parent, cap_std::ambient_authority())
            .with_context(|| format!("Failed to open directory {:?}", parent))?;

        // Get the image digest xattr
        let digest_data = match dir.getxattr(file_name, OsStr::new(BOOTC_IMAGE_DIGEST_XATTR))? {
            Some(data) => data,
            None => {
                tracing::debug!("No image digest xattr found on {:?}", path);
                return Ok(None);
            }
        };

        let digest = std::str::from_utf8(&digest_data)
            .with_context(|| "Invalid UTF-8 in image digest xattr")?;

        tracing::debug!("Read image digest from {:?}: {}", path, digest);
        Ok(Some(digest.to_string()))
    }
}

impl DiskImageMetadata {
    /// Create new metadata from image digest
    pub fn from(_options: &InstallOptions, image: &str) -> Self {
        Self {
            digest: image.to_owned(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ValidationError {
    #[error("file is missing")]
    MissingFile,
    #[error("Missing extended attribute metadata")]
    MissingXattr,
    #[error("Hash mismatch")]
    HashMismatch,
}

/// Check if a cached disk image can be reused by comparing cache hashes
pub fn check_cached_disk(
    path: &Path,
    image_digest: &str,
    install_options: &InstallOptions,
) -> Result<Result<(), ValidationError>> {
    if !path.exists() {
        tracing::debug!("Disk image {:?} does not exist", path);
        return Ok(Err(ValidationError::MissingFile));
    }

    // Compute expected hash directly from install options
    let expected_hash = install_options.compute_hash(image_digest);

    // Read the cache hash from the disk image
    let parent = path
        .parent()
        .ok_or_else(|| color_eyre::eyre::eyre!("Path has no parent directory"))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| color_eyre::eyre::eyre!("Path has no file name"))?;

    let dir = Dir::open_ambient_dir(parent, cap_std::ambient_authority())
        .with_context(|| format!("Failed to open directory {:?}", parent))?;

    let cached_hash = match dir.getxattr(file_name, OsStr::new(BOOTC_CACHE_HASH_XATTR))? {
        Some(data) => std::str::from_utf8(&data)
            .with_context(|| "Invalid UTF-8 in cache hash xattr")?
            .to_string(),
        None => {
            tracing::debug!("No cache hash xattr found on {:?}", path);
            return Ok(Err(ValidationError::MissingXattr));
        }
    };

    let matches = expected_hash == cached_hash;
    if matches {
        tracing::info!(
            "Found cached disk image at {:?} matching cache hash {}",
            path,
            expected_hash
        );
        Ok(Ok(()))
    } else {
        tracing::debug!(
            "Cached disk at {:?} does not match requirements. \
             Expected hash: {}, found: {}",
            path,
            expected_hash,
            cached_hash
        );
        Ok(Err(ValidationError::HashMismatch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_hash_generation() {
        let install_options1 = InstallOptions {
            filesystem: Some("ext4".to_string()),
            root_size: Some("20G".to_string()),
            ..Default::default()
        };

        let install_options2 = InstallOptions {
            filesystem: Some("ext4".to_string()),
            root_size: Some("20G".to_string()),
            ..Default::default()
        };

        // Same inputs should generate same hash
        assert_eq!(
            install_options1.compute_hash("sha256:abc123"),
            install_options2.compute_hash("sha256:abc123")
        );

        // Different image digest should generate different hash
        let install_options3 = InstallOptions {
            filesystem: Some("ext4".to_string()),
            root_size: Some("20G".to_string()),
            ..Default::default()
        };

        assert_ne!(
            install_options1.compute_hash("sha256:abc123"),
            install_options3.compute_hash("sha256:xyz789")
        );

        // Different filesystem should generate different hash
        let install_options4 = InstallOptions {
            filesystem: Some("xfs".to_string()),
            root_size: Some("20G".to_string()),
            ..Default::default()
        };

        assert_ne!(
            install_options1.compute_hash("sha256:abc123"),
            install_options4.compute_hash("sha256:abc123")
        );

        // Different target_transport should generate different hash
        let mut install_options5 = InstallOptions {
            filesystem: Some("ext4".to_string()),
            root_size: Some("20G".to_string()),
            ..Default::default()
        };
        install_options5.target_transport = Some("containers-storage".to_string());

        assert_ne!(
            install_options1.compute_hash("sha256:abc123"),
            install_options5.compute_hash("sha256:abc123")
        );
    }
}
