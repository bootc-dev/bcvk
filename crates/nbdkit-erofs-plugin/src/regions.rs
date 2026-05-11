//! Region-based virtual block device composition.
//! Inspired by the regions pattern in nbdkit's floppy plugin (BSD-3-Clause).

use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum RegionType {
    Data(Arc<Vec<u8>>),
    File { path: PathBuf },
    Zero,
}

#[derive(Debug, Clone)]
pub struct Region {
    pub start: u64,
    pub len: u64,
    pub region_type: RegionType,
}

impl Region {
    pub fn end(&self) -> u64 {
        self.start + self.len
    }
}

pub fn find_region(regions: &[Region], offset: u64) -> Option<&Region> {
    regions
        .binary_search_by(|r| {
            if offset < r.start {
                std::cmp::Ordering::Greater
            } else if offset >= r.end() {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .ok()
        .map(|i| &regions[i])
}

pub fn pread(regions: &[Region], buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    let mut remaining = buf.len();
    let mut buf_offset = 0;
    let mut disk_offset = offset;

    while remaining > 0 {
        let region = find_region(regions, disk_offset).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("offset {} outside disk", disk_offset),
            )
        })?;

        let region_offset = disk_offset - region.start;
        let avail = (region.len - region_offset) as usize;
        let len = remaining.min(avail);

        match &region.region_type {
            RegionType::Data(data) => {
                let start = region_offset as usize;
                buf[buf_offset..buf_offset + len].copy_from_slice(&data[start..start + len]);
            }
            RegionType::File { path } => {
                use std::os::unix::fs::FileExt;
                let f = std::fs::File::open(path)?;
                f.read_exact_at(&mut buf[buf_offset..buf_offset + len], region_offset)?;
            }
            RegionType::Zero => {
                buf[buf_offset..buf_offset + len].fill(0);
            }
        }

        remaining -= len;
        buf_offset += len;
        disk_offset += len as u64;
    }

    Ok(())
}
