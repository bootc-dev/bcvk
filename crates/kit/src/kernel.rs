//! Kernel detection for container images.
//!
//! This module provides functionality to detect kernel and initramfs in container
//! images, supporting traditional kernels, Unified Kernel Images (UKI), and
//! aboot artifacts.

use std::io::Read;
use std::path::Path;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std_ext::cap_std::fs::Dir;
use cap_std_ext::dirext::CapStdExtDirExt;
use color_eyre::eyre::{bail, Context, Result};

/// The EFI Linux directory where UKIs are stored (relative to /boot)
const EFI_LINUX: &str = "EFI/Linux";

/// The modules directory (relative to /usr/lib)
const MODULES_DIR: &str = "modules";

/// UKI file extension
const UKI_EXTENSION: &str = "efi";

/// Traditional kernel filename
const VMLINUZ: &str = "vmlinuz";

/// Traditional initramfs filename
const INITRAMFS: &str = "initramfs.img";

/// Aboot artifacts use this name regardless of whether they contain a UKI or
/// an Android boot image.
const ABOOT_PREFIX: &str = "aboot-";
const ABOOT_EXTENSION: &str = "img";
const ANDROID_BOOT_MAGIC: &[u8; 8] = b"ANDROID!";
const PE_MAGIC: &[u8; 2] = b"MZ";

/// Format of the boot artifact from which QEMU obtains its kernel and initramfs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelKind {
    /// Separate vmlinuz and initramfs files.
    Traditional,
    /// Unified Kernel Image, including ukiboot aboot payloads.
    Uki,
    /// Android boot image containing a kernel and ramdisk.
    AndroidBoot,
}

/// Information about a kernel found in a container image.
#[derive(Debug, Clone)]
pub struct KernelInfo {
    /// Path to the kernel or combined boot artifact.
    pub kernel_path: Utf8PathBuf,
    /// Path to the initramfs (only for traditional kernels).
    pub initramfs_path: Option<Utf8PathBuf>,
    /// Format of the discovered boot artifact.
    pub kind: KernelKind,
}

/// Find kernel/initramfs in a container image root directory.
///
/// Aboot artifacts take precedence over UKIs, which take precedence over
/// traditional kernels. An aboot image may retain other kernel files.
///
/// Search order:
/// 1. `/boot/aboot-*.img` - ukiboot UKI or Android boot image
/// 2. `/boot/EFI/Linux/*.efi` - UKI in ESP
/// 3. `/usr/lib/modules/<version>/*.efi` - UKI alongside modules
/// 4. `/usr/lib/modules/<version>/vmlinuz` + `initramfs.img` - traditional
///
/// Returns an error if multiple artifacts of the preferred kind are found.
/// Returns `None` if no kernel is found.
pub fn find_kernel(root: &Dir) -> Result<Option<KernelInfo>> {
    let aboot = find_aboot_artifacts(root)?;
    match aboot.as_slice() {
        [] => {}
        [artifact] => return Ok(Some(artifact.clone())),
        artifacts => {
            let paths: Vec<_> = artifacts.iter().map(|k| k.kernel_path.as_str()).collect();
            bail!(
                "Found {} aboot artifacts, expected exactly one:\n  {}",
                artifacts.len(),
                paths.join("\n  ")
            );
        }
    }

    // First, collect all UKIs
    let mut ukis: Vec<KernelInfo> = Vec::new();
    ukis.extend(find_ukis_in_esp(root)?);
    ukis.extend(find_ukis_in_modules(root)?);

    // If we have UKIs, require exactly one
    if !ukis.is_empty() {
        return match ukis.len() {
            1 => Ok(ukis.into_iter().next()),
            n => {
                let paths: Vec<_> = ukis.iter().map(|k| k.kernel_path.as_str()).collect();
                bail!(
                    "Found {n} UKIs, expected exactly one:\n  {}",
                    paths.join("\n  ")
                );
            }
        };
    }

    // No UKIs found, look for traditional kernels
    let traditional = find_traditional_kernels_in_modules(root)?;

    match traditional.len() {
        0 => Ok(None),
        1 => Ok(traditional.into_iter().next()),
        n => {
            let paths: Vec<_> = traditional.iter().map(|k| k.kernel_path.as_str()).collect();
            bail!(
                "Found {n} traditional kernels, expected exactly one:\n  {}",
                paths.join("\n  ")
            );
        }
    }
}

fn find_aboot_artifacts(root: &Dir) -> Result<Vec<KernelInfo>> {
    let Some(boot) = root.open_dir_optional("boot")? else {
        return Ok(Vec::new());
    };
    let mut artifacts = Vec::new();
    for entry in boot.entries()? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some((stem, extension)) = name.rsplit_once('.') else {
            continue;
        };
        let Some(version) = stem.strip_prefix(ABOOT_PREFIX) else {
            continue;
        };
        if version.is_empty() || extension != ABOOT_EXTENSION {
            continue;
        }

        let mut file = boot.open(name)?;
        let mut magic = [0u8; 8];
        file.read_exact(&mut magic)
            .with_context(|| format!("reading header of boot/{name}"))?;
        let kind = if &magic == ANDROID_BOOT_MAGIC {
            KernelKind::AndroidBoot
        } else if magic.starts_with(PE_MAGIC) {
            KernelKind::Uki
        } else {
            bail!("Unsupported aboot artifact format: boot/{name}");
        };
        artifacts.push(KernelInfo {
            kernel_path: Utf8PathBuf::from(format!("boot/{name}")),
            initramfs_path: None,
            kind,
        });
    }
    Ok(artifacts)
}

/// Check if a filename has the UKI extension (.efi)
fn is_uki_file(name: &std::ffi::OsStr) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|ext| ext == UKI_EXTENSION)
}

/// Find all UKIs in /boot/EFI/Linux/*.efi
fn find_ukis_in_esp(root: &Dir) -> Result<Vec<KernelInfo>> {
    let Some(boot) = root.open_dir_optional("boot")? else {
        return Ok(Vec::new());
    };
    let Some(efi_linux) = boot.open_dir_optional(EFI_LINUX)? else {
        return Ok(Vec::new());
    };

    let mut ukis = Vec::new();
    for entry in efi_linux.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        if is_uki_file(&name) {
            if let Some(name_str) = name.to_str() {
                ukis.push(KernelInfo {
                    kernel_path: Utf8PathBuf::from(format!("boot/{EFI_LINUX}/{name_str}")),
                    initramfs_path: None,
                    kind: KernelKind::Uki,
                });
            }
        }
    }

    Ok(ukis)
}

/// Open the modules directory, returning None if it doesn't exist
fn open_modules_dir(root: &Dir) -> Result<Option<Dir>> {
    let Some(usr_lib) = root.open_dir_optional("usr/lib")? else {
        return Ok(None);
    };
    Ok(usr_lib.open_dir_optional(MODULES_DIR)?)
}

/// Find all UKIs in /usr/lib/modules/<version>/*.efi
fn find_ukis_in_modules(root: &Dir) -> Result<Vec<KernelInfo>> {
    let Some(modules) = open_modules_dir(root)? else {
        return Ok(Vec::new());
    };

    let mut ukis = Vec::new();

    for entry in modules.entries()? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(version) = entry.file_name().to_str().map(|s| s.to_owned()) else {
            continue;
        };

        let version_dir = modules
            .open_dir(&version)
            .with_context(|| format!("opening modules/{version}"))?;

        for uki_name in find_ukis_in_version_dir(&version_dir)? {
            ukis.push(KernelInfo {
                kernel_path: Utf8PathBuf::from(format!(
                    "usr/lib/{MODULES_DIR}/{version}/{uki_name}"
                )),
                initramfs_path: None,
                kind: KernelKind::Uki,
            });
        }
    }

    Ok(ukis)
}

/// Find all traditional kernels in /usr/lib/modules/<version>/
fn find_traditional_kernels_in_modules(root: &Dir) -> Result<Vec<KernelInfo>> {
    let Some(modules) = open_modules_dir(root)? else {
        return Ok(Vec::new());
    };

    let mut kernels = Vec::new();

    for entry in modules.entries()? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(version) = entry.file_name().to_str().map(|s| s.to_owned()) else {
            continue;
        };

        let version_dir = modules
            .open_dir(&version)
            .with_context(|| format!("opening modules/{version}"))?;

        if has_traditional_kernel(&version_dir) {
            kernels.push(KernelInfo {
                kernel_path: Utf8PathBuf::from(format!(
                    "usr/lib/{MODULES_DIR}/{version}/{VMLINUZ}"
                )),
                initramfs_path: Some(Utf8PathBuf::from(format!(
                    "usr/lib/{MODULES_DIR}/{version}/{INITRAMFS}"
                ))),
                kind: KernelKind::Traditional,
            });
        }
    }

    Ok(kernels)
}

/// Find all UKI (.efi files) in a kernel version directory
fn find_ukis_in_version_dir(version_dir: &Dir) -> Result<Vec<String>> {
    let mut ukis = Vec::new();
    for entry in version_dir.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        if is_uki_file(&name) && entry.file_type()?.is_file() {
            if let Some(name_str) = name.to_str() {
                ukis.push(name_str.to_owned());
            }
        }
    }
    Ok(ukis)
}

/// Check if a version directory has a traditional kernel (vmlinuz + initramfs.img)
fn has_traditional_kernel(version_dir: &Dir) -> bool {
    version_dir.exists(VMLINUZ) && version_dir.exists(INITRAMFS)
}

/// Prepend a root path prefix to a KernelInfo's paths
pub fn with_root_prefix(info: KernelInfo, root: &Utf8Path) -> KernelInfo {
    KernelInfo {
        kernel_path: root.join(&info.kernel_path),
        initramfs_path: info.initramfs_path.map(|p| root.join(&p)),
        kind: info.kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_std_ext::cap_std;
    use cap_std_ext::cap_tempfile;

    #[test]
    fn test_find_kernel_none() -> Result<()> {
        let tempdir = cap_tempfile::tempdir(cap_std::ambient_authority())?;
        assert!(find_kernel(&tempdir)?.is_none());
        Ok(())
    }

    #[test]
    fn test_find_kernel_traditional() -> Result<()> {
        let tempdir = cap_tempfile::tempdir(cap_std::ambient_authority())?;
        tempdir.create_dir_all("usr/lib/modules/6.12.0-100.fc41.x86_64")?;
        tempdir.atomic_write(
            "usr/lib/modules/6.12.0-100.fc41.x86_64/vmlinuz",
            b"fake kernel",
        )?;
        tempdir.atomic_write(
            "usr/lib/modules/6.12.0-100.fc41.x86_64/initramfs.img",
            b"fake initramfs",
        )?;

        let info = find_kernel(&tempdir)?.expect("should find kernel");
        assert_eq!(info.kind, KernelKind::Traditional);
        assert!(info.kernel_path.as_str().contains("vmlinuz"));
        assert!(info.initramfs_path.is_some());
        assert!(info
            .initramfs_path
            .as_ref()
            .unwrap()
            .as_str()
            .contains("initramfs.img"));
        Ok(())
    }

    #[test]
    fn test_find_kernel_uki_in_esp() -> Result<()> {
        let tempdir = cap_tempfile::tempdir(cap_std::ambient_authority())?;
        tempdir.create_dir_all("boot/EFI/Linux")?;
        tempdir.atomic_write("boot/EFI/Linux/fedora-6.12.0.efi", b"fake uki")?;

        let info = find_kernel(&tempdir)?.expect("should find kernel");
        assert_eq!(info.kind, KernelKind::Uki);
        assert!(info.kernel_path.as_str().contains("fedora-6.12.0.efi"));
        assert!(info.initramfs_path.is_none());
        Ok(())
    }

    #[test]
    fn test_find_kernel_uki_in_modules() -> Result<()> {
        let tempdir = cap_tempfile::tempdir(cap_std::ambient_authority())?;
        tempdir.create_dir_all("usr/lib/modules/6.12.0-100.fc41.x86_64")?;
        tempdir.atomic_write(
            "usr/lib/modules/6.12.0-100.fc41.x86_64/6.12.0-100.fc41.x86_64.efi",
            b"fake uki",
        )?;

        let info = find_kernel(&tempdir)?.expect("should find kernel");
        assert_eq!(info.kind, KernelKind::Uki);
        assert!(info
            .kernel_path
            .as_str()
            .contains("6.12.0-100.fc41.x86_64.efi"));
        assert!(info.initramfs_path.is_none());
        Ok(())
    }

    #[test]
    fn test_find_kernel_uki_preferred_over_traditional() -> Result<()> {
        // Old images may have both UKI and vmlinuz - UKI should take precedence
        let tempdir = cap_tempfile::tempdir(cap_std::ambient_authority())?;

        // Traditional kernel in modules
        tempdir.create_dir_all("usr/lib/modules/6.12.0-100.fc41.x86_64")?;
        tempdir.atomic_write(
            "usr/lib/modules/6.12.0-100.fc41.x86_64/vmlinuz",
            b"fake kernel",
        )?;
        tempdir.atomic_write(
            "usr/lib/modules/6.12.0-100.fc41.x86_64/initramfs.img",
            b"fake initramfs",
        )?;

        // UKI in ESP
        tempdir.create_dir_all("boot/EFI/Linux")?;
        tempdir.atomic_write("boot/EFI/Linux/fedora-6.12.0.efi", b"fake uki")?;

        // Should find the UKI, ignoring traditional kernel
        let info = find_kernel(&tempdir)?.expect("should find kernel");
        assert_eq!(info.kind, KernelKind::Uki);
        assert!(info.kernel_path.as_str().contains("fedora-6.12.0.efi"));
        Ok(())
    }

    #[test]
    fn test_find_kernel_uki_preferred_in_same_dir() -> Result<()> {
        // UKI and traditional in same version dir - UKI takes precedence
        let tempdir = cap_tempfile::tempdir(cap_std::ambient_authority())?;
        tempdir.create_dir_all("usr/lib/modules/6.12.0-100.fc41.x86_64")?;

        // Both UKI and traditional in same version dir
        tempdir.atomic_write(
            "usr/lib/modules/6.12.0-100.fc41.x86_64/vmlinuz",
            b"fake kernel",
        )?;
        tempdir.atomic_write(
            "usr/lib/modules/6.12.0-100.fc41.x86_64/initramfs.img",
            b"fake initramfs",
        )?;
        tempdir.atomic_write(
            "usr/lib/modules/6.12.0-100.fc41.x86_64/6.12.0-100.fc41.x86_64.efi",
            b"fake uki",
        )?;

        // Should find the UKI, ignoring traditional kernel
        let info = find_kernel(&tempdir)?.expect("should find kernel");
        assert_eq!(info.kind, KernelKind::Uki);
        assert!(info
            .kernel_path
            .as_str()
            .contains("6.12.0-100.fc41.x86_64.efi"));
        Ok(())
    }

    #[test]
    fn test_find_kernel_multiple_ukis_in_esp_errors() {
        let tempdir = cap_tempfile::tempdir(cap_std::ambient_authority()).unwrap();
        tempdir.create_dir_all("boot/EFI/Linux").unwrap();
        tempdir
            .atomic_write("boot/EFI/Linux/zzz.efi", b"fake uki")
            .unwrap();
        tempdir
            .atomic_write("boot/EFI/Linux/aaa.efi", b"fake uki")
            .unwrap();
        tempdir
            .atomic_write("boot/EFI/Linux/mmm.efi", b"fake uki")
            .unwrap();

        let result = find_kernel(&tempdir);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Found 3 UKIs"));
    }

    #[test]
    fn test_find_kernel_multiple_versions_errors() {
        let tempdir = cap_tempfile::tempdir(cap_std::ambient_authority()).unwrap();

        // Two different kernel versions
        tempdir
            .create_dir_all("usr/lib/modules/6.12.0-100.fc41.x86_64")
            .unwrap();
        tempdir
            .atomic_write(
                "usr/lib/modules/6.12.0-100.fc41.x86_64/vmlinuz",
                b"fake kernel",
            )
            .unwrap();
        tempdir
            .atomic_write(
                "usr/lib/modules/6.12.0-100.fc41.x86_64/initramfs.img",
                b"fake initramfs",
            )
            .unwrap();

        tempdir
            .create_dir_all("usr/lib/modules/6.11.0-50.fc41.x86_64")
            .unwrap();
        tempdir
            .atomic_write(
                "usr/lib/modules/6.11.0-50.fc41.x86_64/vmlinuz",
                b"fake kernel",
            )
            .unwrap();
        tempdir
            .atomic_write(
                "usr/lib/modules/6.11.0-50.fc41.x86_64/initramfs.img",
                b"fake initramfs",
            )
            .unwrap();

        let result = find_kernel(&tempdir);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Found 2 traditional kernels"));
    }

    #[test]
    fn test_find_kernel_multiple_ukis_in_modules_errors() {
        let tempdir = cap_tempfile::tempdir(cap_std::ambient_authority()).unwrap();

        // Two UKIs in different version directories
        tempdir
            .create_dir_all("usr/lib/modules/6.12.0-100.fc41.x86_64")
            .unwrap();
        tempdir
            .atomic_write(
                "usr/lib/modules/6.12.0-100.fc41.x86_64/6.12.0-100.fc41.x86_64.efi",
                b"fake uki",
            )
            .unwrap();

        tempdir
            .create_dir_all("usr/lib/modules/6.11.0-50.fc41.x86_64")
            .unwrap();
        tempdir
            .atomic_write(
                "usr/lib/modules/6.11.0-50.fc41.x86_64/6.11.0-50.fc41.x86_64.efi",
                b"fake uki",
            )
            .unwrap();

        let result = find_kernel(&tempdir);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Found 2 UKIs"));
    }

    #[test]
    fn test_find_aboot_artifact() -> Result<()> {
        for (header, kind) in [
            (b"ANDROID!".as_slice(), KernelKind::AndroidBoot),
            (b"MZ______".as_slice(), KernelKind::Uki),
        ] {
            let root = cap_tempfile::tempdir(cap_std::ambient_authority())?;
            root.create_dir_all("boot")?;
            root.atomic_write("boot/aboot-6.12.img", header)?;
            root.atomic_write("boot/aboot-.img", b"")?;
            root.atomic_write("boot/aboot-6.12.img.bak", b"")?;
            root.atomic_write("boot/vbmeta-6.12.img", b"vbmeta")?;
            root.create_dir_all("usr/lib/modules/6.12")?;
            root.atomic_write("usr/lib/modules/6.12/vmlinuz", b"kernel")?;
            root.atomic_write("usr/lib/modules/6.12/initramfs.img", b"initramfs")?;

            let info = find_kernel(&root)?.expect("aboot artifact should take precedence");
            assert_eq!(info.kind, kind);
            assert_eq!(info.kernel_path, Utf8Path::new("boot/aboot-6.12.img"));
            assert!(info.initramfs_path.is_none());
        }
        Ok(())
    }

    #[test]
    fn test_find_aboot_artifact_errors() -> Result<()> {
        let root = cap_tempfile::tempdir(cap_std::ambient_authority())?;
        root.create_dir_all("boot")?;
        root.atomic_write("boot/aboot-6.12.img", b"unknown!")?;
        assert!(find_kernel(&root)
            .unwrap_err()
            .to_string()
            .contains("Unsupported aboot artifact format"));

        root.atomic_write("boot/aboot-6.12.img", b"ANDROID!")?;
        root.atomic_write("boot/aboot-6.13.img", b"ANDROID!")?;
        assert!(find_kernel(&root)
            .unwrap_err()
            .to_string()
            .contains("Found 2 aboot artifacts"));
        Ok(())
    }

    #[test]
    fn test_with_root_prefix() {
        let info = KernelInfo {
            kernel_path: Utf8PathBuf::from("boot/EFI/Linux/test.efi"),
            initramfs_path: None,
            kind: KernelKind::Uki,
        };

        let prefixed = with_root_prefix(info, Utf8Path::new("/run/source-image"));
        assert_eq!(
            prefixed.kernel_path.as_str(),
            "/run/source-image/boot/EFI/Linux/test.efi"
        );
    }

    #[test]
    fn test_with_root_prefix_traditional() {
        let info = KernelInfo {
            kernel_path: Utf8PathBuf::from("usr/lib/modules/6.12.0/vmlinuz"),
            initramfs_path: Some(Utf8PathBuf::from("usr/lib/modules/6.12.0/initramfs.img")),
            kind: KernelKind::Traditional,
        };

        let prefixed = with_root_prefix(info, Utf8Path::new("/run/source-image"));
        assert_eq!(
            prefixed.kernel_path.as_str(),
            "/run/source-image/usr/lib/modules/6.12.0/vmlinuz"
        );
        assert_eq!(
            prefixed.initramfs_path.as_ref().unwrap().as_str(),
            "/run/source-image/usr/lib/modules/6.12.0/initramfs.img"
        );
    }

    #[test]
    fn test_is_uki_file() {
        use std::ffi::OsStr;
        assert!(is_uki_file(OsStr::new("kernel.efi")));
        assert!(is_uki_file(OsStr::new("6.12.0-100.fc41.x86_64.efi")));
        assert!(!is_uki_file(OsStr::new("vmlinuz")));
        assert!(!is_uki_file(OsStr::new("initramfs.img")));
        assert!(!is_uki_file(OsStr::new("config")));
    }
}
