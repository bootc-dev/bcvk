//! Minimal CPIO archive creation for initramfs appending
//!
//! This module implements the "newc" CPIO format for appending files to
//! an initramfs. The Linux kernel supports concatenating multiple CPIO
//! archives, so we can simply append our files to an existing initramfs.

use std::io::{self, BufWriter, Write};

/// CPIO "newc" format magic number
const CPIO_MAGIC: &str = "070701";

/// Write a CPIO archive entry header
fn write_header<W: Write>(
    writer: &mut BufWriter<W>,
    name: &str,
    mode: u32,
    file_size: u32,
) -> io::Result<()> {
    let name_with_nul = format!("{}\0", name);
    // SAFETY: name length should fit within 32 bits
    let namesize: u32 = name_with_nul.len().try_into().unwrap();

    // newc header format: all fields are 8-char hex ASCII
    let ino = 0u32;
    write!(
        writer,
        "{CPIO_MAGIC}{ino:08x}{mode:08x}{uid:08x}{gid:08x}{nlink:08x}{mtime:08x}{filesize:08x}{devmajor:08x}{devminor:08x}{rdevmajor:08x}{rdevminor:08x}{namesize:08x}{check:08x}",
        uid = 0u32,
        gid = 0u32,
        nlink = 1u32,
        mtime = 0u32,
        filesize = file_size,
        devmajor = 0u32,
        devminor = 0u32,
        rdevmajor = 0u32,
        rdevminor = 0u32,
        check = 0u32,
    )?;

    // Write filename (with NUL terminator)
    writer.write_all(name_with_nul.as_bytes())?;

    // Pad to 4-byte boundary after header + filename
    // Header is 110 bytes, so total is 110 + namesize
    let header_plus_name = 110 + namesize;
    let padding = (4 - (header_plus_name % 4)) % 4;
    for _ in 0..padding {
        writer.write_all(b"\0")?;
    }

    Ok(())
}

/// Pad output to 4-byte boundary
fn write_data_padding<W: Write>(writer: &mut BufWriter<W>, data_len: u32) -> io::Result<()> {
    let padding = (4 - (data_len % 4)) % 4;
    for _ in 0..padding {
        writer.write_all(b"\0")?;
    }
    Ok(())
}

/// Write a directory entry to a CPIO archive
fn write_directory<W: Write>(writer: &mut BufWriter<W>, path: &str) -> io::Result<()> {
    // Directory mode: 0755 + S_IFDIR (0o40000)
    let mode = 0o40755;
    write_header(writer, path, mode, 0)?;
    Ok(())
}

/// Write a regular file entry to a CPIO archive
fn write_file<W: Write>(
    writer: &mut BufWriter<W>,
    path: &str,
    content: &[u8],
    mode: u32,
) -> io::Result<()> {
    // Add S_IFREG (0o100000) to mode
    let full_mode = 0o100000 | mode;
    // SAFETY: content length should fit within 32 bits
    let content_len: u32 = content.len().try_into().unwrap();
    write_header(writer, path, full_mode, content_len)?;
    writer.write_all(content)?;
    write_data_padding(writer, content_len)?;
    Ok(())
}

/// Write the CPIO trailer (end of archive marker)
fn write_trailer<W: Write>(writer: &mut BufWriter<W>) -> io::Result<()> {
    write_header(writer, "TRAILER!!!", 0, 0)?;
    Ok(())
}

/// Create a CPIO archive with bcvk initramfs units
///
/// This creates a minimal CPIO archive containing:
/// - The /etc overlay service unit (runs in initramfs)
/// - The /var ephemeral service unit (runs in initramfs)  
/// - The copy-units service (copies journal-stream to /sysroot/etc for systemd <256)
/// - The journal-stream service (to be copied for systemd <256 compatibility)
/// - Drop-in files to pull units into appropriate targets
///
/// On systemd v256+, the journal-stream unit is created via SMBIOS credentials.
/// On older versions, bcvk-copy-units.service copies the embedded unit to
/// /sysroot/etc/systemd/system/ before switch-root.
pub fn create_initramfs_units_cpio() -> Vec<u8> {
    let mut buf = Vec::new();
    let mut writer = BufWriter::new(&mut buf);

    // Include the initramfs service units
    let etc_overlay_content = include_str!("units/bcvk-etc-overlay.service");
    let var_ephemeral_content = include_str!("units/bcvk-var-ephemeral.service");
    let copy_units_content = include_str!("units/bcvk-copy-units.service");

    // Include the journal-stream service (copied to /sysroot/etc on systemd <256)
    let journal_stream_content = include_str!("units/bcvk-journal-stream.service");

    // Create directory structure
    write_directory(&mut writer, "usr").unwrap();
    write_directory(&mut writer, "usr/lib").unwrap();
    write_directory(&mut writer, "usr/lib/systemd").unwrap();
    write_directory(&mut writer, "usr/lib/systemd/system").unwrap();

    // Write the initramfs service units (mode 0644)
    write_file(
        &mut writer,
        "usr/lib/systemd/system/bcvk-etc-overlay.service",
        etc_overlay_content.as_bytes(),
        0o644,
    )
    .unwrap();

    write_file(
        &mut writer,
        "usr/lib/systemd/system/bcvk-var-ephemeral.service",
        var_ephemeral_content.as_bytes(),
        0o644,
    )
    .unwrap();

    write_file(
        &mut writer,
        "usr/lib/systemd/system/bcvk-copy-units.service",
        copy_units_content.as_bytes(),
        0o644,
    )
    .unwrap();

    // Write the journal-stream service (will be copied to /sysroot/etc on systemd <256)
    write_file(
        &mut writer,
        "usr/lib/systemd/system/bcvk-journal-stream.service",
        journal_stream_content.as_bytes(),
        0o644,
    )
    .unwrap();

    // Create drop-in directories and files to pull units into initrd-fs.target
    write_directory(&mut writer, "usr/lib/systemd/system/initrd-fs.target.d").unwrap();

    let etc_dropin = "[Unit]\nWants=bcvk-etc-overlay.service\n";
    write_file(
        &mut writer,
        "usr/lib/systemd/system/initrd-fs.target.d/bcvk-etc-overlay.conf",
        etc_dropin.as_bytes(),
        0o644,
    )
    .unwrap();

    let var_dropin = "[Unit]\nWants=bcvk-var-ephemeral.service\n";
    write_file(
        &mut writer,
        "usr/lib/systemd/system/initrd-fs.target.d/bcvk-var-ephemeral.conf",
        var_dropin.as_bytes(),
        0o644,
    )
    .unwrap();

    let copy_dropin = "[Unit]\nWants=bcvk-copy-units.service\n";
    write_file(
        &mut writer,
        "usr/lib/systemd/system/initrd-fs.target.d/bcvk-copy-units.conf",
        copy_dropin.as_bytes(),
        0o644,
    )
    .unwrap();

    // Write trailer
    write_trailer(&mut writer).unwrap();

    // Flush and return the buffer
    writer.into_inner().unwrap();
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_initramfs_units_cpio() {
        let cpio = create_initramfs_units_cpio();

        // Should start with CPIO magic
        assert!(cpio.starts_with(CPIO_MAGIC.as_bytes()));

        let cpio_str = std::str::from_utf8(&cpio).unwrap();

        // Should contain the embedded service units
        assert!(cpio_str.contains("bcvk-etc-overlay.service"));
        assert!(cpio_str.contains("bcvk-var-ephemeral.service"));
        assert!(cpio_str.contains("bcvk-copy-units.service"));
        assert!(cpio_str.contains("bcvk-journal-stream.service"));

        // Should contain the drop-in configs
        assert!(cpio_str.contains("initrd-fs.target.d"));

        // Should end with TRAILER!!!
        assert!(cpio_str.contains("TRAILER!!!"));
    }
}
