use crate::dir_walk::{ChildRef, DirInfo, WalkResult};
use crate::regions::{Region, RegionType};
use std::sync::Arc;

const EROFS_MAGIC: u32 = 0xE0F5E1E2;
const BLOCK_SIZE: u64 = 4096;
const BLOCK_BITS: u8 = 12;
const SUPERBLOCK_OFFSET: u64 = 1024;

// EROFS inode formats
const EROFS_INODE_LAYOUT_COMPACT: u16 = 0;

// EROFS data layouts
const EROFS_INODE_FLAT_PLAIN: u16 = 0;

// EROFS file types (matching Linux DT_* values)
const EROFS_FT_REG_FILE: u8 = 1;
const EROFS_FT_DIR: u8 = 2;
const EROFS_FT_SYMLINK: u8 = 7;

#[derive(Debug)]
pub struct FileRegion {
    pub file_index: usize,
    pub offset_in_erofs: u64,
    pub size: u64,
}

#[derive(Debug)]
pub struct ErofsLayout {
    pub metadata: Vec<u8>,
    pub file_regions: Vec<FileRegion>,
    pub total_size: u64,
}

struct DirEntryOnDisk {
    nid: u64,
    file_type: u8,
    name: Vec<u8>,
}

pub fn build_erofs(walk: &WalkResult) -> std::io::Result<ErofsLayout> {
    let total_inodes = walk.dirs.len() + walk.files.len() + walk.symlinks.len();

    // Phase 1: Assign inode positions
    // Inodes start at block 1 (block 0 has superblock)
    let inode_table_offset = BLOCK_SIZE; // block 1
    let inode_size: u64 = 32; // compact inode
    let inode_table_size = align_up(total_inodes as u64 * inode_size, BLOCK_SIZE);

    // Phase 2: Build directory entry blocks
    let dir_blocks_offset = inode_table_offset + inode_table_size;
    let mut dir_data: Vec<u8> = Vec::new();
    let mut dir_block_offsets: Vec<u64> = Vec::new(); // per-directory offset in dir_data

    for dir in &walk.dirs {
        let offset = align_up(dir_data.len() as u64, BLOCK_SIZE);
        dir_data.resize(offset as usize, 0);
        dir_block_offsets.push(dir_blocks_offset + offset);

        let mut entries = Vec::new();

        // "." entry
        entries.push(DirEntryOnDisk {
            nid: dir.inode_id,
            file_type: EROFS_FT_DIR,
            name: b".".to_vec(),
        });

        // ".." entry (root points to self)
        let parent_nid = dir.parent_inode_id;
        entries.push(DirEntryOnDisk {
            nid: parent_nid,
            file_type: EROFS_FT_DIR,
            name: b"..".to_vec(),
        });

        // children (sorted by name in walk)
        for child in &dir.children {
            match child {
                ChildRef::Dir(di) => {
                    let child_dir = &walk.dirs[*di];
                    entries.push(DirEntryOnDisk {
                        nid: child_dir.inode_id,
                        file_type: EROFS_FT_DIR,
                        name: child_dir.name.as_encoded_bytes().to_vec(),
                    });
                }
                ChildRef::File(fi) => {
                    let file = &walk.files[*fi];
                    entries.push(DirEntryOnDisk {
                        nid: file.inode_id,
                        file_type: EROFS_FT_REG_FILE,
                        name: file
                            .host_path
                            .file_name()
                            .unwrap_or_default()
                            .as_encoded_bytes()
                            .to_vec(),
                    });
                }
                ChildRef::Symlink(si) => {
                    let symlink = &walk.symlinks[*si];
                    entries.push(DirEntryOnDisk {
                        nid: symlink.inode_id,
                        file_type: EROFS_FT_SYMLINK,
                        name: symlink.name.clone(),
                    });
                }
            }
        }

        // Write EROFS directory blocks (splits at 4096-byte boundaries)
        write_dir_blocks(&mut dir_data, &entries);
    }
    let dir_data_size = align_up(dir_data.len() as u64, BLOCK_SIZE);
    dir_data.resize(dir_data_size as usize, 0);

    // Phase 3: Compute data region layout
    let data_offset = dir_blocks_offset + dir_data_size;
    let mut file_regions = Vec::new();
    let mut current_data_offset = data_offset;

    for (i, file) in walk.files.iter().enumerate() {
        if file.size > 0 {
            let aligned_offset = align_up(current_data_offset, BLOCK_SIZE);
            file_regions.push(FileRegion {
                file_index: i,
                offset_in_erofs: aligned_offset,
                size: file.size,
            });
            current_data_offset = aligned_offset + align_up(file.size, BLOCK_SIZE);
        }
    }

    // Symlink targets also need data blocks
    for (si, symlink) in walk.symlinks.iter().enumerate() {
        if !symlink.target.is_empty() {
            let aligned_offset = align_up(current_data_offset, BLOCK_SIZE);
            file_regions.push(FileRegion {
                file_index: walk.files.len() + si, // files.len() + symlink index
                offset_in_erofs: aligned_offset,
                size: symlink.target.len() as u64,
            });
            current_data_offset =
                aligned_offset + align_up(symlink.target.len() as u64, BLOCK_SIZE);
        }
    }

    let total_size = align_up(current_data_offset, BLOCK_SIZE);
    let total_blocks = total_size / BLOCK_SIZE;

    // Phase 4: Build metadata blob
    let mut metadata = vec![0u8; (dir_blocks_offset + dir_data_size) as usize];

    // Write superblock at offset 1024
    write_superblock(
        &mut metadata,
        total_inodes as u32,
        total_blocks as u32,
        0, // root nid
    );

    // Write inodes
    // Directories
    for (i, dir) in walk.dirs.iter().enumerate() {
        let dir_size = compute_dir_size(dir, walk);
        let dir_block = (dir_block_offsets[i] - dir_blocks_offset) / BLOCK_SIZE;
        write_compact_inode(
            &mut metadata,
            inode_table_offset as usize + (dir.inode_id as usize * 32),
            0o040000 | (dir.mode & 0o7777),
            dir.uid as u16,
            dir.gid as u16,
            dir_size as u32,
            dir.mtime as u32,
            2 + dir
                .children
                .iter()
                .filter(|c| matches!(c, ChildRef::Dir(_)))
                .count() as u16,
            EROFS_INODE_FLAT_PLAIN,
            (dir_blocks_offset / BLOCK_SIZE + dir_block) as u32,
        );
    }

    // Regular files
    for (i, file) in walk.files.iter().enumerate() {
        let data_block = if file.size > 0 {
            let fr = file_regions.iter().find(|r| r.file_index == i);
            fr.map(|r| (r.offset_in_erofs / BLOCK_SIZE) as u32)
                .unwrap_or(0)
        } else {
            0
        };
        write_compact_inode(
            &mut metadata,
            inode_table_offset as usize + (file.inode_id as usize * 32),
            0o100000 | (file.mode & 0o7777),
            file.uid as u16,
            file.gid as u16,
            file.size as u32,
            file.mtime as u32,
            file.nlink as u16,
            EROFS_INODE_FLAT_PLAIN,
            data_block,
        );
    }

    // Symlinks: FlatPlain with target in data region
    // File regions for symlinks start after file regions
    let file_region_count = walk.files.iter().filter(|f| f.size > 0).count();
    let mut sym_fr_idx = file_region_count;
    for symlink in &walk.symlinks {
        let data_block = if !symlink.target.is_empty() {
            let fr = &file_regions[sym_fr_idx];
            sym_fr_idx += 1;
            (fr.offset_in_erofs / BLOCK_SIZE) as u32
        } else {
            0
        };

        write_compact_inode(
            &mut metadata,
            inode_table_offset as usize + (symlink.inode_id as usize * 32),
            0o120000 | (symlink.mode & 0o7777),
            symlink.uid as u16,
            symlink.gid as u16,
            symlink.target.len() as u32,
            symlink.mtime as u32,
            1,
            EROFS_INODE_FLAT_PLAIN,
            data_block,
        );
    }

    // Write directory data
    let dir_start = dir_blocks_offset as usize;
    if dir_start + dir_data.len() <= metadata.len() {
        metadata[dir_start..dir_start + dir_data.len()].copy_from_slice(&dir_data);
    }

    Ok(ErofsLayout {
        metadata,
        file_regions,
        total_size,
    })
}

fn write_superblock(buf: &mut [u8], inodes: u32, blocks: u32, root_nid: u16) {
    let off = SUPERBLOCK_OFFSET as usize;
    // magic
    buf[off..off + 4].copy_from_slice(&EROFS_MAGIC.to_le_bytes());
    // checksum (unused)
    // feature_compat
    buf[off + 8..off + 12].copy_from_slice(&0u32.to_le_bytes());
    // blkszbits
    buf[off + 12] = BLOCK_BITS;
    // sb_extslots
    buf[off + 13] = 0;
    // root_nid
    buf[off + 14..off + 16].copy_from_slice(&root_nid.to_le_bytes());
    // inos
    buf[off + 16..off + 24].copy_from_slice(&(inodes as u64).to_le_bytes());
    // build_time
    buf[off + 24..off + 32].copy_from_slice(&0u64.to_le_bytes());
    // build_time_nsec
    buf[off + 32..off + 36].copy_from_slice(&0u32.to_le_bytes());
    // blocks
    buf[off + 36..off + 40].copy_from_slice(&blocks.to_le_bytes());
    // meta_blkaddr (inode table starts at block 1)
    buf[off + 40..off + 44].copy_from_slice(&1u32.to_le_bytes());
    // xattr_blkaddr
    buf[off + 44..off + 48].copy_from_slice(&0u32.to_le_bytes());
    // uuid (16 bytes)
    // volume_name (16 bytes)
    // feature_incompat
    buf[off + 80..off + 84].copy_from_slice(&0u32.to_le_bytes());
    // available_compr_algs (union with checksum)
    // lz4_max_distance
}

fn write_compact_inode(
    buf: &mut [u8],
    offset: usize,
    mode: u32,
    uid: u16,
    gid: u16,
    size: u32,
    _mtime: u32,
    nlink: u16,
    data_layout: u16,
    u_field: u32,
) {
    if offset + 32 > buf.len() {
        return;
    }

    // format: layout(compact=0) | data_layout << 1
    let format = (EROFS_INODE_LAYOUT_COMPACT) | (data_layout << 1);
    buf[offset..offset + 2].copy_from_slice(&format.to_le_bytes());
    // xattr_icount
    buf[offset + 2..offset + 4].copy_from_slice(&0u16.to_le_bytes());
    // mode
    buf[offset + 4..offset + 6].copy_from_slice(&(mode as u16).to_le_bytes());
    // nlink
    buf[offset + 6..offset + 8].copy_from_slice(&nlink.to_le_bytes());
    // size
    buf[offset + 8..offset + 12].copy_from_slice(&size.to_le_bytes());
    // reserved
    buf[offset + 12..offset + 16].copy_from_slice(&0u32.to_le_bytes());
    // u (union: raw_blkaddr for FlatPlain)
    buf[offset + 16..offset + 20].copy_from_slice(&u_field.to_le_bytes());
    // ino (on-disk inode number, optional)
    buf[offset + 20..offset + 24].copy_from_slice(&0u32.to_le_bytes());
    // uid
    buf[offset + 24..offset + 26].copy_from_slice(&uid.to_le_bytes());
    // gid
    buf[offset + 26..offset + 28].copy_from_slice(&gid.to_le_bytes());
    // reserved2
    buf[offset + 28..offset + 32].copy_from_slice(&0u32.to_le_bytes());
}

fn write_dir_blocks(buf: &mut Vec<u8>, entries: &[DirEntryOnDisk]) {
    // EROFS directories are split into 4096-byte blocks.
    // Each block contains: [headers...][names...]
    // header = 12 bytes: nid(8) + nameoff(2) + file_type(1) + reserved(1)
    // nameoff is relative to block start.

    let mut remaining = entries;

    while !remaining.is_empty() {
        // Determine how many entries fit in this block
        let mut count = 0;
        let mut total_size: usize = 0;
        for entry in remaining {
            let entry_size = 12 + entry.name.len();
            if total_size + entry_size > BLOCK_SIZE as usize && count > 0 {
                break;
            }
            total_size += entry_size;
            count += 1;
        }

        let block_entries = &remaining[..count];
        remaining = &remaining[count..];

        // Write headers
        let header_total = 12 * block_entries.len();
        let mut nameoff = header_total as u16;
        for entry in block_entries {
            buf.extend_from_slice(&(entry.nid as u64).to_le_bytes());
            buf.extend_from_slice(&nameoff.to_le_bytes());
            buf.push(entry.file_type);
            buf.push(0);
            nameoff += entry.name.len() as u16;
        }

        // Write names
        for entry in block_entries {
            buf.extend_from_slice(&entry.name);
        }

        // Pad to block boundary (except last block which is sized by inode.size)
        if !remaining.is_empty() {
            let written = total_size;
            let pad = BLOCK_SIZE as usize - (written % BLOCK_SIZE as usize);
            if pad < BLOCK_SIZE as usize {
                buf.resize(buf.len() + pad, 0);
            }
        }
    }
}

fn compute_dir_size(dir: &DirInfo, walk: &WalkResult) -> u64 {
    // Build entry list to accurately compute size including block splits
    let mut entries = Vec::new();
    entries.push(DirEntryOnDisk {
        nid: 0,
        file_type: EROFS_FT_DIR,
        name: b".".to_vec(),
    });
    entries.push(DirEntryOnDisk {
        nid: 0,
        file_type: EROFS_FT_DIR,
        name: b"..".to_vec(),
    });
    for child in &dir.children {
        let name_len = match child {
            ChildRef::Dir(di) => walk.dirs[*di].name.len(),
            ChildRef::File(fi) => walk.files[*fi]
                .host_path
                .file_name()
                .unwrap_or_default()
                .len(),
            ChildRef::Symlink(si) => walk.symlinks[*si].name.len(),
        };
        entries.push(DirEntryOnDisk {
            nid: 0,
            file_type: 0,
            name: vec![0; name_len],
        });
    }

    // Simulate block splitting to get total size
    let mut total = 0u64;
    let mut remaining = &entries[..];
    while !remaining.is_empty() {
        let mut count = 0;
        let mut block_size = 0usize;
        for entry in remaining {
            let entry_size = 12 + entry.name.len();
            if block_size + entry_size > BLOCK_SIZE as usize && count > 0 {
                break;
            }
            block_size += entry_size;
            count += 1;
        }
        remaining = &remaining[count..];
        if remaining.is_empty() {
            total += block_size as u64; // last block: actual size
        } else {
            total += BLOCK_SIZE; // full block
        }
    }
    total
}

fn align_up(val: u64, align: u64) -> u64 {
    (val + align - 1) & !(align - 1)
}

pub fn build_erofs_regions(layout: &ErofsLayout, walk: &WalkResult) -> Vec<Region> {
    let files = &walk.files;
    let mut regions = Vec::new();

    // Metadata region (superblock + inode table + dir blocks)
    regions.push(Region {
        start: 0,
        len: layout.metadata.len() as u64,
        region_type: RegionType::Data(Arc::new(layout.metadata.clone())),
    });

    // File and symlink data regions
    for fr in &layout.file_regions {
        // Padding gap
        let current_end = regions.last().map(|r| r.start + r.len).unwrap_or(0);
        if fr.offset_in_erofs > current_end {
            regions.push(Region {
                start: current_end,
                len: fr.offset_in_erofs - current_end,
                region_type: RegionType::Zero,
            });
        }

        if fr.file_index < files.len() {
            // Regular file: read from host
            regions.push(Region {
                start: fr.offset_in_erofs,
                len: fr.size,
                region_type: RegionType::File {
                    path: files[fr.file_index].host_path.clone(),
                },
            });
        } else {
            // Symlink target: inline data
            let sym_idx = fr.file_index - files.len();
            if sym_idx < walk.symlinks.len() {
                // Pad symlink target to fill the block
                let mut data = walk.symlinks[sym_idx].target.clone();
                data.resize(fr.size as usize, 0);
                regions.push(Region {
                    start: fr.offset_in_erofs,
                    len: fr.size,
                    region_type: RegionType::Data(Arc::new(data)),
                });
            }
        }

        // Padding to block boundary
        let end = fr.offset_in_erofs + fr.size;
        let aligned_end = align_up(end, BLOCK_SIZE);
        if aligned_end > end {
            regions.push(Region {
                start: end,
                len: aligned_end - end,
                region_type: RegionType::Zero,
            });
        }
    }

    // Ensure total size
    let last_end = regions.last().map(|r| r.start + r.len).unwrap_or(0);
    if last_end < layout.total_size {
        regions.push(Region {
            start: last_end,
            len: layout.total_size - last_end,
            region_type: RegionType::Zero,
        });
    }

    regions
}
