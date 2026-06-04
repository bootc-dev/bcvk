use crate::regions::{Region, RegionType};
use std::sync::Arc;

const SECTOR_SIZE: u64 = 512;
const GPT_HEADER_SIZE: u64 = 92;
const GPT_ENTRY_SIZE: u64 = 128;
const GPT_ENTRIES: u64 = 128;

// EFI System Partition type GUID
const ESP_TYPE_GUID: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

// Linux filesystem type GUID
const LINUX_TYPE_GUID: [u8; 16] = [
    0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47, 0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D, 0xE4,
];

pub struct DiskLayout {
    pub regions: Vec<Region>,
    pub total_size: u64,
}

pub fn build_gpt_disk(
    esp_regions: Vec<Region>,
    esp_size: u64,
    erofs_regions: Vec<Region>,
    erofs_size: u64,
) -> std::io::Result<DiskLayout> {
    // GPT layout:
    // LBA 0: Protective MBR
    // LBA 1: GPT Header
    // LBA 2-33: Partition Table (128 entries * 128 bytes = 16384 bytes = 32 sectors)
    // LBA 34+: ESP partition (aligned to 2048 sectors / 1MB)
    // After ESP: EROFS partition
    // End: Backup GPT

    let partition_table_sectors = (GPT_ENTRIES * GPT_ENTRY_SIZE + SECTOR_SIZE - 1) / SECTOR_SIZE;
    let first_usable_lba = 34u64; // standard
    let esp_start_lba = 2048u64; // 1MB aligned
    let esp_sectors = (esp_size + SECTOR_SIZE - 1) / SECTOR_SIZE;
    let erofs_start_lba = esp_start_lba + esp_sectors;
    // Align to 2048 sectors
    let erofs_start_lba = (erofs_start_lba + 2047) & !2047;
    let erofs_sectors = (erofs_size + SECTOR_SIZE - 1) / SECTOR_SIZE;
    let last_usable_lba = erofs_start_lba + erofs_sectors - 1;
    let backup_table_lba = last_usable_lba + 1;
    let backup_header_lba = backup_table_lba + partition_table_sectors;
    let total_sectors = backup_header_lba + 1;
    let total_size = total_sectors * SECTOR_SIZE;

    // Build partition table entries
    let mut partition_table = vec![0u8; (GPT_ENTRIES * GPT_ENTRY_SIZE) as usize];

    // Entry 0: ESP
    write_gpt_entry(
        &mut partition_table,
        0,
        &ESP_TYPE_GUID,
        esp_start_lba,
        esp_start_lba + esp_sectors - 1,
        b"EFI System",
    );

    // Entry 1: EROFS rootfs
    write_gpt_entry(
        &mut partition_table,
        1,
        &LINUX_TYPE_GUID,
        erofs_start_lba,
        erofs_start_lba + erofs_sectors - 1,
        b"bcvk-root",
    );

    let partition_table_crc = crc32fast::hash(&partition_table);

    // Build GPT header
    let mut gpt_header = vec![0u8; SECTOR_SIZE as usize];
    write_gpt_header(
        &mut gpt_header,
        1, // my LBA
        backup_header_lba,
        first_usable_lba,
        last_usable_lba,
        2, // partition table LBA
        2, // num entries used
        partition_table_crc,
    );

    // Build backup GPT header
    let mut backup_header = vec![0u8; SECTOR_SIZE as usize];
    write_gpt_header(
        &mut backup_header,
        backup_header_lba,
        1, // alternate LBA
        first_usable_lba,
        last_usable_lba,
        backup_table_lba,
        2,
        partition_table_crc,
    );

    // Build protective MBR
    let mut mbr = vec![0u8; SECTOR_SIZE as usize];
    write_protective_mbr(&mut mbr, total_sectors);

    // Assemble regions
    let mut regions = Vec::new();

    // MBR
    regions.push(Region {
        start: 0,
        len: SECTOR_SIZE,
        region_type: RegionType::Data(Arc::new(mbr)),
    });

    // GPT Header
    regions.push(Region {
        start: SECTOR_SIZE,
        len: SECTOR_SIZE,
        region_type: RegionType::Data(Arc::new(gpt_header)),
    });

    // Partition Table
    regions.push(Region {
        start: 2 * SECTOR_SIZE,
        len: partition_table.len() as u64,
        region_type: RegionType::Data(Arc::new(partition_table.clone())),
    });

    // Padding to ESP start
    let pad_start = 2 * SECTOR_SIZE + partition_table.len() as u64;
    let esp_byte_offset = esp_start_lba * SECTOR_SIZE;
    if esp_byte_offset > pad_start {
        regions.push(Region {
            start: pad_start,
            len: esp_byte_offset - pad_start,
            region_type: RegionType::Zero,
        });
    }

    // ESP partition (from provided regions, offset-adjusted)
    for mut r in esp_regions {
        r.start += esp_byte_offset;
        regions.push(r);
    }

    // Padding between ESP and EROFS
    let esp_end = esp_byte_offset + esp_size;
    let erofs_byte_offset = erofs_start_lba * SECTOR_SIZE;
    if erofs_byte_offset > esp_end {
        regions.push(Region {
            start: esp_end,
            len: erofs_byte_offset - esp_end,
            region_type: RegionType::Zero,
        });
    }

    // EROFS partition (offset all regions)
    for mut r in erofs_regions {
        r.start += erofs_byte_offset;
        regions.push(r);
    }

    // Padding to backup GPT
    let erofs_end = erofs_byte_offset + erofs_size;
    let backup_table_offset = backup_table_lba * SECTOR_SIZE;
    if backup_table_offset > erofs_end {
        regions.push(Region {
            start: erofs_end,
            len: backup_table_offset - erofs_end,
            region_type: RegionType::Zero,
        });
    }

    // Backup partition table
    regions.push(Region {
        start: backup_table_offset,
        len: partition_table.len() as u64,
        region_type: RegionType::Data(Arc::new(partition_table)),
    });

    // Backup GPT header
    regions.push(Region {
        start: backup_header_lba * SECTOR_SIZE,
        len: SECTOR_SIZE,
        region_type: RegionType::Data(Arc::new(backup_header)),
    });

    Ok(DiskLayout {
        regions,
        total_size,
    })
}

fn write_gpt_entry(
    table: &mut [u8],
    index: usize,
    type_guid: &[u8; 16],
    first_lba: u64,
    last_lba: u64,
    name: &[u8],
) {
    let off = index * GPT_ENTRY_SIZE as usize;
    // Partition type GUID
    table[off..off + 16].copy_from_slice(type_guid);
    // Unique partition GUID (generate simple one from index)
    let mut unique = [0u8; 16];
    unique[0] = index as u8 + 1;
    unique[15] = 0x42;
    table[off + 16..off + 32].copy_from_slice(&unique);
    // First LBA
    table[off + 32..off + 40].copy_from_slice(&first_lba.to_le_bytes());
    // Last LBA
    table[off + 40..off + 48].copy_from_slice(&last_lba.to_le_bytes());
    // Attributes
    table[off + 48..off + 56].copy_from_slice(&0u64.to_le_bytes());
    // Name (UTF-16LE)
    for (i, &b) in name.iter().enumerate().take(36) {
        table[off + 56 + i * 2] = b;
        table[off + 56 + i * 2 + 1] = 0;
    }
}

fn write_gpt_header(
    buf: &mut [u8],
    my_lba: u64,
    alternate_lba: u64,
    first_usable: u64,
    last_usable: u64,
    partition_table_lba: u64,
    _num_entries: u32,
    partition_crc: u32,
) {
    // Signature "EFI PART"
    buf[0..8].copy_from_slice(b"EFI PART");
    // Revision 1.0
    buf[8..12].copy_from_slice(&0x00010000u32.to_le_bytes());
    // Header size
    buf[12..16].copy_from_slice(&(GPT_HEADER_SIZE as u32).to_le_bytes());
    // Header CRC32 (computed after all fields set)
    // My LBA
    buf[24..32].copy_from_slice(&my_lba.to_le_bytes());
    // Alternate LBA
    buf[32..40].copy_from_slice(&alternate_lba.to_le_bytes());
    // First usable LBA
    buf[40..48].copy_from_slice(&first_usable.to_le_bytes());
    // Last usable LBA
    buf[48..56].copy_from_slice(&last_usable.to_le_bytes());
    // Fixed disk GUID for reproducible builds (not security-sensitive)
    const DISK_GUID: [u8; 16] = [
        0xAA, 0xBB, 0xCC, 0xDD, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB,
        0xCC,
    ];
    let disk_guid = DISK_GUID;
    buf[56..72].copy_from_slice(&disk_guid);
    // Partition entry start LBA
    buf[72..80].copy_from_slice(&partition_table_lba.to_le_bytes());
    // Number of partition entries
    buf[80..84].copy_from_slice(&(GPT_ENTRIES as u32).to_le_bytes());
    // Size of partition entry
    buf[84..88].copy_from_slice(&(GPT_ENTRY_SIZE as u32).to_le_bytes());
    // Partition table CRC32
    buf[88..92].copy_from_slice(&partition_crc.to_le_bytes());

    // Compute header CRC32
    buf[16..20].copy_from_slice(&0u32.to_le_bytes()); // zero CRC field first
    let crc = crc32fast::hash(&buf[0..GPT_HEADER_SIZE as usize]);
    buf[16..20].copy_from_slice(&crc.to_le_bytes());
}

fn write_protective_mbr(buf: &mut [u8], total_sectors: u64) {
    // Partition entry at offset 446
    buf[446] = 0x00; // not bootable
    buf[447] = 0x00; // CHS start
    buf[448] = 0x02;
    buf[449] = 0x00;
    buf[450] = 0xEE; // type: GPT protective
    buf[451] = 0xFF; // CHS end
    buf[452] = 0xFF;
    buf[453] = 0xFF;
    // LBA start
    buf[454..458].copy_from_slice(&1u32.to_le_bytes());
    // LBA size
    let size = std::cmp::min(total_sectors - 1, 0xFFFFFFFF) as u32;
    buf[458..462].copy_from_slice(&size.to_le_bytes());
    // Boot signature
    buf[510] = 0x55;
    buf[511] = 0xAA;
}
