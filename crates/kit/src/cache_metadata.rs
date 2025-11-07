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
use cap_std_ext::cap_std::fs::Dir;
use cap_std_ext::dirext::CapStdExtDirExt;
use color_eyre::{eyre::Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::File;

/// Extended attribute name for storing bootc cache hash
const BOOTC_CACHE_HASH_XATTR: &str = "user.bootc.cache_hash";

/// Extended attribute name for storing container image digest
const BOOTC_IMAGE_DIGEST_XATTR: &str = "user.bootc.image_digest";

/// Build inputs used to generate a cache hash
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheInputs {
    /// SHA256 digest of the source container image
    image_digest: String,

    /// Filesystem type used for installation (e.g., "ext4", "xfs", "btrfs")
    filesystem: Option<String>,

    /// Root filesystem size if specified
    root_size: Option<String>,

    /// Whether to use composefs-native storage
    composefs_backend: bool,

    /// Kernel arguments used during installation
    kernel_args: Vec<String>,

    /// Version of the cache format for future compatibility
    version: u32,
}

/// Metadata stored on disk images for caching purposes
#[derive(Debug, Clone)]
pub struct DiskImageMetadata {
    /// SHA256 digest of the source container image
    pub digest: String,

    /// Filesystem type used for installation (e.g., "ext4", "xfs", "btrfs")
    pub filesystem: Option<String>,

    /// Root filesystem size if specified
    pub root_size: Option<String>,

    /// Whether to use composefs-native storage
    pub composefs_backend: bool,

    /// Kernel arguments used during installation
    pub kernel_args: Vec<String>,

    /// Version of the metadata format for future compatibility
    pub version: u32,
}

impl DiskImageMetadata {
    /// Generate SHA256 hash of all build inputs
    pub fn compute_cache_hash(&self) -> String {
        let inputs = CacheInputs {
            image_digest: self.digest.clone(),
            filesystem: self.filesystem.clone(),
            root_size: self.root_size.clone(),
            composefs_backend: self.composefs_backend,
            kernel_args: self.kernel_args.clone(),
            version: self.version,
        };

        let json = serde_json::to_string(&inputs).expect("Failed to serialize cache inputs");
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        format!("sha256:{:x}", hasher.finalize())
    }

    /// Write metadata to a file using extended attributes via rustix
    pub fn write_to_file(&self, file: &File) -> Result<()> {
        // Write the cache hash
        let cache_hash = self.compute_cache_hash();
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

    /// Read image digest from a file using extended attributes
    pub fn read_image_digest_from_path(dir: &Dir, file_name: &OsStr) -> Result<Option<String>> {
        // Get the image digest xattr
        let digest_data = match dir.getxattr(file_name, OsStr::new(BOOTC_IMAGE_DIGEST_XATTR))? {
            Some(data) => data,
            None => {
                tracing::debug!("No image digest xattr found on {:?}", file_name);
                return Ok(None);
            }
        };

        let digest = std::str::from_utf8(&digest_data)
            .with_context(|| "Invalid UTF-8 in image digest xattr")?;

        tracing::debug!("Read image digest from {:?}: {}", file_name, digest);
        Ok(Some(digest.to_string()))
    }
}

impl DiskImageMetadata {
    /// Create new metadata from InstallOptions and image digest
    pub fn from(options: &InstallOptions, image: &str) -> Self {
        Self {
            version: 1,
            digest: image.to_owned(),
            filesystem: options.filesystem.clone(),
            root_size: options.root_size.clone(),
            kernel_args: options.karg.clone(),
            composefs_backend: options.composefs_backend,
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
    dir: &Dir,
    file_name: impl AsRef<OsStr>,
    image_digest: &str,
    install_options: &InstallOptions,
) -> Result<Result<(), ValidationError>> {
    let file_name = file_name.as_ref();

    if !dir.try_exists(file_name)? {
        tracing::debug!("Disk image {:?} does not exist", file_name);
        return Ok(Err(ValidationError::MissingFile));
    }

    // Create metadata for the current request to compute expected hash
    let expected_meta = DiskImageMetadata::from(install_options, image_digest);
    let expected_hash = expected_meta.compute_cache_hash();

    // Read the cache hash from the disk image
    let cached_hash = match dir.getxattr(file_name, OsStr::new(BOOTC_CACHE_HASH_XATTR))? {
        Some(data) => std::str::from_utf8(&data)
            .with_context(|| "Invalid UTF-8 in cache hash xattr")?
            .to_string(),
        None => {
            tracing::debug!("No cache hash xattr found on {:?}", file_name);
            return Ok(Err(ValidationError::MissingXattr));
        }
    };

    let matches = expected_hash == cached_hash;
    if matches {
        tracing::debug!(
            "Found cached disk image {:?} matching cache hash {}",
            file_name,
            expected_hash
        );
        Ok(Ok(()))
    } else {
        tracing::debug!(
            "Cached disk {:?} does not match requirements. \
             Expected hash: {}, found: {}",
            file_name,
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
        let metadata1 = DiskImageMetadata::from(&install_options1, "sha256:abc123");

        let install_options2 = InstallOptions {
            filesystem: Some("ext4".to_string()),
            root_size: Some("20G".to_string()),
            ..Default::default()
        };
        let metadata2 = DiskImageMetadata::from(&install_options2, "sha256:abc123");

        // Same inputs should generate same hash
        assert_eq!(
            metadata1.compute_cache_hash(),
            metadata2.compute_cache_hash()
        );

        // Different inputs should generate different hashes
        let install_options3 = InstallOptions {
            filesystem: Some("ext4".to_string()),
            root_size: Some("20G".to_string()),
            ..Default::default()
        };
        let metadata3 = DiskImageMetadata::from(&install_options3, "sha256:xyz789");

        assert_ne!(
            metadata1.compute_cache_hash(),
            metadata3.compute_cache_hash()
        );

        // Different filesystem should generate different hash
        let install_options4 = InstallOptions {
            filesystem: Some("xfs".to_string()),
            root_size: Some("20G".to_string()),
            ..Default::default()
        };
        let metadata4 = DiskImageMetadata::from(&install_options4, "sha256:abc123");

        assert_ne!(
            metadata1.compute_cache_hash(),
            metadata4.compute_cache_hash()
        );
    }

    #[test]
    fn test_cache_inputs_serialization() -> Result<()> {
        let inputs = CacheInputs {
            image_digest: "sha256:abc123".to_string(),
            filesystem: Some("ext4".to_string()),
            root_size: Some("20G".to_string()),
            kernel_args: vec!["console=ttyS0".to_string()],
            composefs_backend: false,
            version: 1,
        };

        let json = serde_json::to_string(&inputs)?;
        let deserialized: CacheInputs = serde_json::from_str(&json)?;

        assert_eq!(inputs.image_digest, deserialized.image_digest);
        assert_eq!(inputs.filesystem, deserialized.filesystem);
        assert_eq!(inputs.root_size, deserialized.root_size);
        assert_eq!(inputs.kernel_args, deserialized.kernel_args);
        assert_eq!(inputs.version, deserialized.version);
        Ok(())
    }
}
