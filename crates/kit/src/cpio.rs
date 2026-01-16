//! CPIO archive creation for initramfs appending.
//!
//! The Linux kernel supports concatenating multiple CPIO archives,
//! so we can simply append our files to an existing initramfs.

use std::io::{self, Write};

use cpio::newc::Builder as NewcBuilder;
use cpio::newc::ModeFileType;

fn write_directory(writer: &mut impl Write, path: &str) -> io::Result<()> {
    let builder = NewcBuilder::new(path)
        .mode(0o755)
        .set_mode_file_type(ModeFileType::Directory);
    builder.write(writer, 0).finish()?;
    Ok(())
}

fn write_file(writer: &mut impl Write, path: &str, content: &[u8]) -> io::Result<()> {
    let builder = NewcBuilder::new(path)
        .mode(0o644)
        .set_mode_file_type(ModeFileType::Regular);
    let mut cpio_writer = builder.write(writer, content.len() as u32);
    cpio_writer.write_all(content)?;
    cpio_writer.finish()?;
    Ok(())
}

/// CPIO entry: either a directory or a file with content.
enum Entry {
    Dir(&'static str),
    File(&'static str, &'static [u8]),
}

/// Create a CPIO archive with bcvk initramfs units for ephemeral VM setup.
pub fn create_initramfs_units_cpio() -> io::Result<Vec<u8>> {
    use Entry::*;

    const UNIT_DIR: &str = "usr/lib/systemd/system";
    const DROPIN_DIR: &str = "usr/lib/systemd/system/initrd-fs.target.d";

    let entries: &[Entry] = &[
        // Directory hierarchy
        Dir("usr"),
        Dir("usr/lib"),
        Dir("usr/lib/systemd"),
        Dir(UNIT_DIR),
        Dir(DROPIN_DIR),
        // Service units
        File(
            "usr/lib/systemd/system/bcvk-etc-overlay.service",
            include_bytes!("units/bcvk-etc-overlay.service"),
        ),
        File(
            "usr/lib/systemd/system/bcvk-var-ephemeral.service",
            include_bytes!("units/bcvk-var-ephemeral.service"),
        ),
        File(
            "usr/lib/systemd/system/bcvk-copy-units.service",
            include_bytes!("units/bcvk-copy-units.service"),
        ),
        File(
            "usr/lib/systemd/system/bcvk-journal-stream.service",
            include_bytes!("units/bcvk-journal-stream.service"),
        ),
        // Drop-in configs to pull units into initrd-fs.target
        File(
            "usr/lib/systemd/system/initrd-fs.target.d/bcvk-etc-overlay.conf",
            b"[Unit]\nWants=bcvk-etc-overlay.service\n",
        ),
        File(
            "usr/lib/systemd/system/initrd-fs.target.d/bcvk-var-ephemeral.conf",
            b"[Unit]\nWants=bcvk-var-ephemeral.service\n",
        ),
        File(
            "usr/lib/systemd/system/initrd-fs.target.d/bcvk-copy-units.conf",
            b"[Unit]\nWants=bcvk-copy-units.service\n",
        ),
    ];

    let mut buf = Vec::new();
    for entry in entries {
        match entry {
            Dir(path) => write_directory(&mut buf, path)?,
            File(path, content) => write_file(&mut buf, path, content)?,
        }
    }

    cpio::newc::trailer(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    #[test]
    fn test_cpio_archive_structure_and_contents() {
        let cpio_data = create_initramfs_units_cpio().unwrap();
        let mut cursor = Cursor::new(cpio_data);

        let mut entries = Vec::new();
        let mut etc_overlay_content = None;

        loop {
            let mut reader = cpio::NewcReader::new(cursor).expect("failed to read CPIO entry");
            if reader.entry().is_trailer() {
                break;
            }

            let name = reader.entry().name().to_string();
            let size = reader.entry().file_size() as usize;
            let mode = reader.entry().mode();

            // Read file content for verification
            if name == "usr/lib/systemd/system/bcvk-etc-overlay.service" {
                let mut content = vec![0u8; size];
                reader
                    .read_exact(&mut content)
                    .expect("failed to read file content");
                etc_overlay_content = Some(String::from_utf8(content).expect("invalid UTF-8"));
            }

            entries.push((name, size, mode));
            cursor = reader.finish().expect("failed to finish entry");
        }

        let names: Vec<_> = entries.iter().map(|(n, _, _)| n.as_str()).collect();

        // Verify directories
        assert!(names.contains(&"usr"));
        assert!(names.contains(&"usr/lib"));
        assert!(names.contains(&"usr/lib/systemd"));
        assert!(names.contains(&"usr/lib/systemd/system"));
        assert!(names.contains(&"usr/lib/systemd/system/initrd-fs.target.d"));

        // Verify service files
        assert!(names.contains(&"usr/lib/systemd/system/bcvk-etc-overlay.service"));
        assert!(names.contains(&"usr/lib/systemd/system/bcvk-var-ephemeral.service"));
        assert!(names.contains(&"usr/lib/systemd/system/bcvk-copy-units.service"));
        assert!(names.contains(&"usr/lib/systemd/system/bcvk-journal-stream.service"));

        // Verify drop-in configs
        assert!(names.contains(&"usr/lib/systemd/system/initrd-fs.target.d/bcvk-etc-overlay.conf"));
        assert!(
            names.contains(&"usr/lib/systemd/system/initrd-fs.target.d/bcvk-var-ephemeral.conf")
        );
        assert!(names.contains(&"usr/lib/systemd/system/initrd-fs.target.d/bcvk-copy-units.conf"));

        // Verify file modes
        for (name, _size, mode) in &entries {
            let file_type = *mode & 0o170000;
            if name.ends_with(".service") || name.ends_with(".conf") {
                assert_eq!(file_type, 0o100000, "{} should be regular file", name);
            } else {
                assert_eq!(file_type, 0o040000, "{} should be directory", name);
            }
        }

        // Verify file content is valid systemd unit
        let content = etc_overlay_content.expect("bcvk-etc-overlay.service not found");
        assert!(content.contains("[Unit]"));
        assert!(content.contains("[Service]"));
        assert!(content.contains("overlay"));
    }
}
