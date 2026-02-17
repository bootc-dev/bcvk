//! Integration tests for anaconda install command
//!
//! These tests verify the anaconda installation workflow using a custom
//! anaconda container image that runs anaconda via systemd.
//!
//! **PREREQUISITES:**
//! - The anaconda-bootc container must be built first:
//!   `podman build -t localhost/anaconda-bootc:latest containers/anaconda-bootc/`
//! - A bootc image must be available in local container storage
//!
//! **NOTE:** These tests are skipped if the anaconda container is not available.

use camino::Utf8PathBuf;
use color_eyre::Result;
use integration_tests::integration_test;
use tempfile::TempDir;
use xshell::cmd;

use crate::{get_bck_command, get_test_image, shell};

const ANACONDA_IMAGE: &str = "localhost/anaconda-bootc:latest";

/// Check if the anaconda container image is available
fn anaconda_image_available() -> bool {
    let sh = match shell() {
        Ok(sh) => sh,
        Err(_) => return false,
    };
    cmd!(sh, "podman image exists {ANACONDA_IMAGE}")
        .quiet()
        .run()
        .is_ok()
}

/// Create a kickstart file for BIOS boot testing
///
/// This kickstart:
/// - Targets specifically the virtio-output disk (ignoring the swap disk)
/// - Uses reqpart to create required boot partitions (biosboot + /boot)
fn create_test_kickstart(dir: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let ks_path = dir.join("test.ks");
    let ks_content = r#"# Test kickstart for bcvk anaconda integration tests (BIOS boot)
text
lang en_US.UTF-8
keyboard us
timezone UTC --utc
network --bootproto=dhcp --activate

# Target only the output disk, ignore the swap disk
ignoredisk --only-use=/dev/disk/by-id/virtio-output

zerombr
clearpart --all --initlabel

# Let anaconda create required boot partitions (biosboot + /boot for BIOS+GPT)
reqpart --add-boot

# Root partition
part / --fstype=xfs --grow

rootpw --lock

poweroff
"#;
    std::fs::write(&ks_path, ks_content)?;
    Ok(ks_path)
}

/// Test anaconda installation to a disk image
///
/// This test requires the anaconda-bootc container to be pre-built.
fn test_anaconda_install() -> Result<()> {
    if !anaconda_image_available() {
        eprintln!(
            "Skipping test_anaconda_install: {} not available",
            ANACONDA_IMAGE
        );
        eprintln!(
            "Build it with: podman build -t {} containers/anaconda-bootc/",
            ANACONDA_IMAGE
        );
        return Ok(());
    }

    let sh = shell()?;
    let bck = get_bck_command()?;
    let image = get_test_image();

    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let disk_path = Utf8PathBuf::try_from(temp_dir.path().join("anaconda-test.img"))
        .expect("temp path is not UTF-8");
    let ks_path = create_test_kickstart(temp_dir.path()).expect("Failed to create kickstart");
    let ks_path_str = ks_path.to_string_lossy().into_owned();

    // Run anaconda install (--no-repoint since we're just testing installation)
    cmd!(
        sh,
        "{bck} anaconda install --kickstart {ks_path_str} --disk-size 10G --no-repoint {image} {disk_path}"
    )
    .run()?;

    // Check that the disk was created
    let metadata = std::fs::metadata(&disk_path).expect("Failed to get disk metadata");
    assert!(
        metadata.len() > 0,
        "test_anaconda_install: Disk image is empty"
    );

    // Mount the disk in an ephemeral VM and verify the installation
    // This is more robust than parsing partition tables or MBR bytes
    //
    // Write the verification script to a directory (--bind requires a directory)
    let verify_dir = temp_dir.path().join("verify");
    std::fs::create_dir(&verify_dir)?;
    let verify_script_path = verify_dir.join("verify.sh");
    let verify_script = r#"#!/bin/bash
set -euo pipefail

# Find the root partition (the largest partition, typically part3)
# Use TYPE=part to filter out the whole disk device
ROOT_PART=$(lsblk -nlo NAME,SIZE,TYPE /dev/disk/by-id/virtio-testdisk | awk '$3=="part"' | sort -k2 -h | tail -1 | awk '{print $1}')
ROOT_DEV="/dev/${ROOT_PART}"

echo "Mounting root partition: ${ROOT_DEV}"
mkdir -p /mnt/testdisk
mount "${ROOT_DEV}" /mnt/testdisk

# Mount the boot partition if it exists (typically vda2)
# The root's /boot is usually a separate partition in anaconda installs
BOOT_PART=$(lsblk -nlo NAME,SIZE,TYPE /dev/disk/by-id/virtio-testdisk | awk '$3=="part"' | sort -k2 -h | sed -n '2p' | awk '{print $1}')
if [ -n "$BOOT_PART" ] && [ -d /mnt/testdisk/boot ]; then
    echo "Mounting boot partition: /dev/${BOOT_PART}"
    mount "/dev/${BOOT_PART}" /mnt/testdisk/boot || true
fi

# Verify ostree deployment exists
if [ ! -d /mnt/testdisk/ostree/deploy ]; then
    echo "FAIL: No ostree deployment found"
    exit 1
fi
echo "OK: ostree deployment exists"

# Verify deployment directory exists
DEPLOY_DIR=$(ls -d /mnt/testdisk/ostree/deploy/*/deploy/*/ 2>/dev/null | head -1)
if [ -z "$DEPLOY_DIR" ]; then
    echo "FAIL: No deployment directory found"
    exit 1
fi
echo "OK: deployment directory found"

# Check for boot loader entries
if ! ls /mnt/testdisk/boot/loader/entries/*.conf >/dev/null 2>&1; then
    if ! ls /mnt/testdisk/boot/loader.*/entries/*.conf >/dev/null 2>&1; then
        echo "FAIL: No boot loader entries found"
        ls -la /mnt/testdisk/boot/ || true
        exit 1
    fi
fi
echo "OK: boot loader entries found"

# Verify /usr/bin exists in the deployment (basic sanity check)
if [ ! -d "${DEPLOY_DIR}/usr/bin" ]; then
    echo "FAIL: deployment /usr/bin not found"
    exit 1
fi
echo "OK: deployment looks valid"

umount /mnt/testdisk/boot 2>/dev/null || true
umount /mnt/testdisk
echo "PASS: anaconda installation verified successfully"
"#;
    std::fs::write(&verify_script_path, verify_script)?;

    // Bind mount the script directory and execute via bash
    let verify_dir_str = verify_dir.to_string_lossy().into_owned();
    let execute_cmd = "bash /run/virtiofs-mnt-verify/verify.sh";
    let output = cmd!(
        sh,
        "{bck} ephemeral run --mount-disk-file {disk_path}:testdisk --bind {verify_dir_str}:verify --execute {execute_cmd} {image}"
    )
    .read()?;

    assert!(
        output.contains("PASS: anaconda installation verified successfully"),
        "test_anaconda_install: Disk verification failed. Output:\n{}",
        output
    );

    Ok(())
}
integration_test!(test_anaconda_install);
