//! FAT32 ESP generation using the regions pattern.
//!
//! Generates a virtual FAT32 filesystem with boot files for EFI boot.
//! Metadata (BPB, FAT tables, directory entries) are in-memory Data regions.
//! File data uses File regions for lazy pread from source files.

use crate::regions::{Region, RegionType};
use std::path::PathBuf;
use std::sync::Arc;

const SECTOR_SIZE: u64 = 512;
const CLUSTER_SIZE: u64 = 512;
const SECTORS_PER_CLUSTER: u64 = 1;
const RESERVED_SECTORS: u64 = 32;
const NUM_FATS: u64 = 2;
const DIR_ENTRY_SIZE: u64 = 32;

const FAT32_EOC: u32 = 0x0FFF_FFFF;
const FAT32_MEDIA: u32 = 0x0FFF_FFF8;

// Fixed cluster assignments for the ESP directory structure.
// Root directory is always cluster 2 per FAT32 spec.
const CLUSTER_ROOT: u32 = 2;
const CLUSTER_EFI: u32 = 3;
const CLUSTER_EFI_BOOT: u32 = 4;
const CLUSTER_BOOT: u32 = 5;

struct FatFile {
    name_8_3: [u8; 11],
    size: u64,
    regions: Vec<FileDataRegion>,
}

pub enum FileDataRegion {
    FromFile { path: PathBuf, len: u64 },
    FromData(Vec<u8>),
    Zero(u64),
}

struct FatDir {
    name_8_3: [u8; 11],
    cluster: u32,
    entries: Vec<FatDirChild>,
}

enum FatDirChild {
    Dir(usize),
    File(usize),
}

fn clusters_for(size: u64) -> u64 {
    if size == 0 {
        1
    } else {
        (size + CLUSTER_SIZE - 1) / CLUSTER_SIZE
    }
}

fn make_8_3(name: &str, ext: &str) -> [u8; 11] {
    let mut r = [b' '; 11];
    for (i, b) in name.bytes().take(8).enumerate() {
        r[i] = b;
    }
    for (i, b) in ext.bytes().take(3).enumerate() {
        r[8 + i] = b;
    }
    r
}

pub fn build_esp_regions(
    grub_path: &std::path::Path,
    grub_size: u64,
    grub_cfg: &[u8],
    kernel_path: &std::path::Path,
    kernel_size: u64,
    initrd_parts: Vec<(FileDataRegion, u64)>,
    initrd_total_size: u64,
) -> (Vec<Region>, u64) {
    // Files
    let mut files: Vec<FatFile> = Vec::new();

    let boot_efi_name = if grub_path
        .file_name()
        .map(|n| n == "grubx64.efi")
        .unwrap_or(false)
    {
        "BOOTX64"
    } else {
        "BOOTAA64"
    };
    files.push(FatFile {
        name_8_3: make_8_3(boot_efi_name, "EFI"),
        size: grub_size,
        regions: vec![FileDataRegion::FromFile {
            path: grub_path.to_path_buf(),
            len: grub_size,
        }],
    });

    // GRUB.CFG
    files.push(FatFile {
        name_8_3: make_8_3("GRUB", "CFG"),
        size: grub_cfg.len() as u64,
        regions: vec![FileDataRegion::FromData(grub_cfg.to_vec())],
    });

    // VMLINUZ
    files.push(FatFile {
        name_8_3: make_8_3("VMLINUZ", ""),
        size: kernel_size,
        regions: vec![FileDataRegion::FromFile {
            path: kernel_path.to_path_buf(),
            len: kernel_size,
        }],
    });

    // INITRD.IMG
    files.push(FatFile {
        name_8_3: make_8_3("INITRD", "IMG"),
        size: initrd_total_size,
        regions: initrd_parts.into_iter().map(|(r, _)| r).collect(),
    });

    // Directory structure:
    //   / (root, cluster 2) → EFI/, boot/
    //   /EFI (cluster 3) → BOOT/
    //   /EFI/BOOT (cluster 4) → BOOTAA64.EFI, GRUB.CFG
    //   /boot (cluster 5) → VMLINUZ, INITRD.IMG
    // Note: /EFI/BOOT and /boot both use 8.3 name "BOOT" but are in different
    // parent directories so there is no conflict in the FAT32 namespace.
    let dirs = vec![
        FatDir {
            name_8_3: make_8_3("", ""),
            cluster: CLUSTER_ROOT,
            entries: vec![FatDirChild::Dir(1), FatDirChild::Dir(3)],
        },
        FatDir {
            name_8_3: make_8_3("EFI", ""),
            cluster: CLUSTER_EFI,
            entries: vec![FatDirChild::Dir(2)],
        },
        FatDir {
            name_8_3: make_8_3("BOOT", ""),
            cluster: CLUSTER_EFI_BOOT,
            entries: vec![FatDirChild::File(0), FatDirChild::File(1)],
        },
        FatDir {
            name_8_3: make_8_3("BOOT", ""),
            cluster: CLUSTER_BOOT,
            entries: vec![FatDirChild::File(2), FatDirChild::File(3)],
        },
    ];

    let dir_clusters = dirs.len() as u32;

    // Assign file clusters (starting after directory clusters)
    let mut file_start_clusters: Vec<u32> = Vec::new();
    let mut next_cluster = 2 + dir_clusters;
    for f in &files {
        file_start_clusters.push(next_cluster);
        next_cluster += clusters_for(f.size) as u32;
    }
    let total_clusters = next_cluster;
    let data_clusters = total_clusters - 2;

    // FAT table
    let fat_entries = total_clusters as usize;
    let fat_bytes = ((fat_entries * 4 + SECTOR_SIZE as usize - 1) / SECTOR_SIZE as usize)
        * SECTOR_SIZE as usize;
    let fat_sectors = fat_bytes as u64 / SECTOR_SIZE;

    let mut fat = vec![0u8; fat_bytes];
    // Entry 0: media descriptor
    fat[0..4].copy_from_slice(&FAT32_MEDIA.to_le_bytes());
    // Entry 1: EOC
    fat[4..8].copy_from_slice(&FAT32_EOC.to_le_bytes());

    // Directory clusters (each is single-cluster, EOC)
    for d in &dirs {
        let off = d.cluster as usize * 4;
        fat[off..off + 4].copy_from_slice(&FAT32_EOC.to_le_bytes());
    }

    // File cluster chains
    for (fi, f) in files.iter().enumerate() {
        let start = file_start_clusters[fi];
        let num = clusters_for(f.size) as u32;
        for c in 0..num {
            let cluster = start + c;
            let off = cluster as usize * 4;
            if c == num - 1 {
                fat[off..off + 4].copy_from_slice(&FAT32_EOC.to_le_bytes());
            } else {
                fat[off..off + 4].copy_from_slice(&(cluster + 1).to_le_bytes());
            }
        }
    }

    // Data region start (in sectors)
    let data_start_sector = RESERVED_SECTORS + NUM_FATS * fat_sectors;

    // Build directory entry blocks
    let mut dir_blocks: Vec<Vec<u8>> = Vec::new();
    for (di, d) in dirs.iter().enumerate() {
        let mut block = vec![0u8; CLUSTER_SIZE as usize];
        let mut pos = 0usize;

        // "." and ".." entries for subdirectories
        if di > 0 {
            write_dir_entry(&mut block, pos, b".          ", 0x10, d.cluster, 0);
            pos += DIR_ENTRY_SIZE as usize;
            // Parent cluster: dirs at index 1 (EFI) and 3 (boot) are children of root (0).
            // Dir at index 2 (EFI/BOOT) is a child of EFI (dirs[1]).
            debug_assert!(dirs.len() == 4, "directory structure changed");
            let parent_cluster = if di == 1 || di == 3 {
                0u32
            } else {
                dirs[1].cluster
            };
            write_dir_entry(&mut block, pos, b"..         ", 0x10, parent_cluster, 0);
            pos += DIR_ENTRY_SIZE as usize;
        }

        for child in &d.entries {
            match child {
                FatDirChild::Dir(idx) => {
                    let cd = &dirs[*idx];
                    write_dir_entry(&mut block, pos, &cd.name_8_3, 0x10, cd.cluster, 0);
                }
                FatDirChild::File(idx) => {
                    let cf = &files[*idx];
                    write_dir_entry(
                        &mut block,
                        pos,
                        &cf.name_8_3,
                        0x20,
                        file_start_clusters[*idx],
                        cf.size,
                    );
                }
            }
            pos += DIR_ENTRY_SIZE as usize;
        }
        dir_blocks.push(block);
    }

    // Total size of ESP partition
    let total_sectors = data_start_sector + data_clusters as u64 * SECTORS_PER_CLUSTER;
    let total_size = total_sectors * SECTOR_SIZE;

    // BPB (Boot Parameter Block)
    let bpb = build_bpb(
        total_sectors as u32,
        fat_sectors as u32,
        data_clusters as u64,
    );

    // FSInfo
    let fsinfo = build_fsinfo(
        (data_clusters as u32).saturating_sub(
            dir_clusters as u32
                + files
                    .iter()
                    .map(|f| clusters_for(f.size) as u32)
                    .sum::<u32>(),
        ),
        next_cluster,
    );

    // Assemble regions
    let mut regions: Vec<Region> = Vec::new();
    let mut offset = 0u64;

    // Sector 0: BPB
    regions.push(Region {
        start: offset,
        len: SECTOR_SIZE,
        region_type: RegionType::Data(Arc::new(bpb.clone())),
    });
    offset += SECTOR_SIZE;

    // Sector 1: FSInfo
    regions.push(Region {
        start: offset,
        len: SECTOR_SIZE,
        region_type: RegionType::Data(Arc::new(fsinfo.clone())),
    });
    offset += SECTOR_SIZE;

    // Sectors 2-5: zero padding
    let pad_to_backup = 4 * SECTOR_SIZE;
    regions.push(Region {
        start: offset,
        len: pad_to_backup,
        region_type: RegionType::Zero,
    });
    offset += pad_to_backup;

    // Sector 6: Backup BPB
    regions.push(Region {
        start: offset,
        len: SECTOR_SIZE,
        region_type: RegionType::Data(Arc::new(bpb)),
    });
    offset += SECTOR_SIZE;

    // Sector 7: Backup FSInfo
    regions.push(Region {
        start: offset,
        len: SECTOR_SIZE,
        region_type: RegionType::Data(Arc::new(fsinfo)),
    });
    offset += SECTOR_SIZE;

    // Sectors 8-31: zero padding to reserved end
    let remaining_reserved = (RESERVED_SECTORS * SECTOR_SIZE) - offset;
    if remaining_reserved > 0 {
        regions.push(Region {
            start: offset,
            len: remaining_reserved,
            region_type: RegionType::Zero,
        });
        offset += remaining_reserved;
    }

    // FAT1
    let fat_data = Arc::new(fat.clone());
    regions.push(Region {
        start: offset,
        len: fat_bytes as u64,
        region_type: RegionType::Data(fat_data.clone()),
    });
    offset += fat_bytes as u64;

    // FAT2 (copy)
    regions.push(Region {
        start: offset,
        len: fat_bytes as u64,
        region_type: RegionType::Data(fat_data),
    });
    offset += fat_bytes as u64;

    // Data area: directory clusters
    for block in &dir_blocks {
        regions.push(Region {
            start: offset,
            len: CLUSTER_SIZE,
            region_type: RegionType::Data(Arc::new(block.clone())),
        });
        offset += CLUSTER_SIZE;
    }

    // Data area: file clusters
    for (_fi, f) in files.iter().enumerate() {
        let mut file_offset = 0u64;

        for part in &f.regions {
            match part {
                FileDataRegion::FromFile { path, len } => {
                    regions.push(Region {
                        start: offset,
                        len: *len,
                        region_type: RegionType::FilePath { path: path.clone() },
                    });
                    offset += len;
                    file_offset += len;
                }
                FileDataRegion::FromData(data) => {
                    let len = data.len() as u64;
                    regions.push(Region {
                        start: offset,
                        len,
                        region_type: RegionType::Data(Arc::new(data.clone())),
                    });
                    offset += len;
                    file_offset += len;
                }
                FileDataRegion::Zero(len) => {
                    if *len > 0 {
                        regions.push(Region {
                            start: offset,
                            len: *len,
                            region_type: RegionType::Zero,
                        });
                        offset += len;
                        file_offset += len;
                    }
                }
            }
        }

        // Pad to cluster boundary
        let used_in_last = file_offset % CLUSTER_SIZE;
        if used_in_last > 0 {
            let pad = CLUSTER_SIZE - used_in_last;
            regions.push(Region {
                start: offset,
                len: pad,
                region_type: RegionType::Zero,
            });
            offset += pad;
        }
    }

    // Ensure total_size is correct
    debug_assert!(
        offset <= total_size,
        "regions exceeded total_size: {} > {}",
        offset,
        total_size
    );
    if offset < total_size {
        regions.push(Region {
            start: offset,
            len: total_size - offset,
            region_type: RegionType::Zero,
        });
    }

    (regions, total_size)
}

/// Build initrd regions: original file + 4-byte alignment + CPIO data.
pub fn build_initrd_regions(
    initrd_path: &std::path::Path,
    initrd_size: u64,
    units_cpio: &[u8],
    ssh_cpio: Option<&[u8]>,
) -> (Vec<(FileDataRegion, u64)>, u64) {
    let mut parts = Vec::new();
    let mut total = 0u64;

    // Original initramfs
    parts.push((
        FileDataRegion::FromFile {
            path: initrd_path.to_path_buf(),
            len: initrd_size,
        },
        initrd_size,
    ));
    total += initrd_size;

    // 4-byte alignment padding
    let pad = ((4 - (initrd_size % 4)) % 4) as u64;
    if pad > 0 {
        parts.push((FileDataRegion::Zero(pad), pad));
        total += pad;
    }

    // Units CPIO
    let len = units_cpio.len() as u64;
    parts.push((FileDataRegion::FromData(units_cpio.to_vec()), len));
    total += len;

    // SSH CPIO (if provided)
    if let Some(ssh) = ssh_cpio {
        let pad2 = ((4 - (total % 4)) % 4) as u64;
        if pad2 > 0 {
            parts.push((FileDataRegion::Zero(pad2), pad2));
            total += pad2;
        }
        let len = ssh.len() as u64;
        parts.push((FileDataRegion::FromData(ssh.to_vec()), len));
        total += len;
    }

    (parts, total)
}

fn write_dir_entry(buf: &mut [u8], pos: usize, name: &[u8; 11], attr: u8, cluster: u32, size: u64) {
    buf[pos..pos + 11].copy_from_slice(name);
    buf[pos + 11] = attr;
    // cluster high
    buf[pos + 20..pos + 22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
    // cluster low
    buf[pos + 26..pos + 28].copy_from_slice(&(cluster as u16).to_le_bytes());
    // file size (32-bit)
    buf[pos + 28..pos + 32].copy_from_slice(&(size as u32).to_le_bytes());
}

fn build_bpb(total_sectors: u32, fat_sectors: u32, _data_clusters: u64) -> Vec<u8> {
    let mut bpb = vec![0u8; SECTOR_SIZE as usize];
    // Jump instruction
    bpb[0] = 0xEB;
    bpb[1] = 0x58;
    bpb[2] = 0x90;
    // OEM name
    bpb[3..11].copy_from_slice(b"MSWIN4.1");
    // Bytes per sector
    bpb[11..13].copy_from_slice(&(SECTOR_SIZE as u16).to_le_bytes());
    // Sectors per cluster
    bpb[13] = SECTORS_PER_CLUSTER as u8;
    // Reserved sectors
    bpb[14..16].copy_from_slice(&(RESERVED_SECTORS as u16).to_le_bytes());
    // Number of FATs
    bpb[16] = NUM_FATS as u8;
    // Root entry count (0 for FAT32)
    bpb[17..19].copy_from_slice(&0u16.to_le_bytes());
    // Total sectors 16 (0 for FAT32)
    bpb[19..21].copy_from_slice(&0u16.to_le_bytes());
    // Media type
    bpb[21] = 0xF8;
    // Sectors per FAT 16 (0 for FAT32)
    bpb[22..24].copy_from_slice(&0u16.to_le_bytes());
    // Sectors per track
    bpb[24..26].copy_from_slice(&32u16.to_le_bytes());
    // Number of heads
    bpb[26..28].copy_from_slice(&64u16.to_le_bytes());
    // Hidden sectors
    bpb[28..32].copy_from_slice(&0u32.to_le_bytes());
    // Total sectors 32
    bpb[32..36].copy_from_slice(&total_sectors.to_le_bytes());
    // --- FAT32 specific ---
    // Sectors per FAT
    bpb[36..40].copy_from_slice(&fat_sectors.to_le_bytes());
    // Extended flags
    bpb[40..42].copy_from_slice(&0u16.to_le_bytes());
    // FS version
    bpb[42..44].copy_from_slice(&0u16.to_le_bytes());
    // Root cluster
    bpb[44..48].copy_from_slice(&2u32.to_le_bytes());
    // FSInfo sector
    bpb[48..50].copy_from_slice(&1u16.to_le_bytes());
    // Backup boot sector
    bpb[50..52].copy_from_slice(&6u16.to_le_bytes());
    // Reserved (12 bytes, already zero)
    // Drive number
    bpb[64] = 0x80;
    // Boot signature
    bpb[66] = 0x29;
    // Volume serial number
    bpb[67..71].copy_from_slice(&0x42424242u32.to_le_bytes());
    // Volume label
    bpb[71..82].copy_from_slice(b"BCVK-ESP   ");
    // Filesystem type
    bpb[82..90].copy_from_slice(b"FAT32   ");
    // Boot signature
    bpb[510] = 0x55;
    bpb[511] = 0xAA;
    bpb
}

fn build_fsinfo(free_clusters: u32, next_free: u32) -> Vec<u8> {
    let mut fs = vec![0u8; SECTOR_SIZE as usize];
    // Signature1
    fs[0..4].copy_from_slice(&0x41615252u32.to_le_bytes());
    // Signature2
    fs[484..488].copy_from_slice(&0x61417272u32.to_le_bytes());
    // Free cluster count
    fs[488..492].copy_from_slice(&free_clusters.to_le_bytes());
    // Next free cluster
    fs[492..496].copy_from_slice(&next_free.to_le_bytes());
    // Signature3
    fs[508..512].copy_from_slice(&0xAA550000u32.to_le_bytes());
    fs
}
