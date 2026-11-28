use crate::{gpt, partition::Partition, BlockDevice, BlockError};
use std::sync::{Arc, Mutex};

struct Disk {
    sector: usize,
    sectors: u64,
    table: Vec<u8>,
    writes: Arc<Mutex<Vec<u64>>>,
}

impl BlockDevice for Disk {
    fn block_size(&self) -> usize { self.sector }
    fn num_blocks(&self) -> u64 { self.sectors }
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let offset = lba as usize * self.sector;
        buf.copy_from_slice(&self.table[offset..offset + self.sector]);
        Ok(())
    }
    fn write_block(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        assert_eq!(buf.len(), self.sector);
        self.writes.lock().unwrap().push(lba);
        Ok(())
    }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}

#[test]
fn native_gpt_entries_and_partition_end() {
    let unique = [0x34; 16];
    for sector in [512, 4096] {
        const ENTRY_BYTES: usize = 128 * 128;
        let mut table = vec![0; 2 * sector + ENTRY_BYTES];
        // The selected entry occupies a different table LBA on 512 and 4096.
        let entries = &mut table[2 * sector..2 * sector + ENTRY_BYTES];
        entries[7 * 128..7 * 128 + 16].copy_from_slice(&gpt::SYSTEM_TYPE);
        entries[7 * 128 + 16..7 * 128 + 32].copy_from_slice(&unique);
        entries[7 * 128 + 32..7 * 128 + 40].copy_from_slice(&40u64.to_le_bytes());
        entries[7 * 128 + 40..7 * 128 + 48].copy_from_slice(&51u64.to_le_bytes());
        let entries_crc = gpt::crc32(entries);
        let header = &mut table[sector..2 * sector];
        header[..8].copy_from_slice(b"EFI PART");
        header[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        header[12..16].copy_from_slice(&92u32.to_le_bytes());
        header[24..32].copy_from_slice(&1u64.to_le_bytes());
        header[40..48].copy_from_slice(&40u64.to_le_bytes());
        header[48..56].copy_from_slice(&89u64.to_le_bytes());
        header[72..80].copy_from_slice(&2u64.to_le_bytes());
        header[80..84].copy_from_slice(&128u32.to_le_bytes());
        header[84..88].copy_from_slice(&128u32.to_le_bytes());
        header[88..92].copy_from_slice(&entries_crc.to_le_bytes());
        let crc = gpt::crc32(&header[..92]);
        header[16..20].copy_from_slice(&crc.to_le_bytes());
        let header = gpt::header(header, 128).unwrap();

        let writes = Arc::new(Mutex::new(Vec::new()));
        let disk = Disk { sector, sectors: 128, table, writes: writes.clone() };
        let entry = gpt::find(&header, &unique, |lba, buf| {
            disk.read_block(lba, buf).map_err(|_| "fixture I/O")
        }).unwrap().unwrap();
        let part = Partition::new(Box::new(disk), entry.first, entry.sectors()).unwrap();

        assert_eq!(part.block_size(), sector);
        let payload = vec![0; 2 * sector];
        assert_eq!(part.write_blocks(10, 2, &payload), Ok(()));
        assert_eq!(part.write_blocks(11, 2, &payload), Err(BlockError::OutOfBounds));
        assert_eq!(part.write_blocks(u64::MAX, 2, &payload), Err(BlockError::OutOfBounds));
        assert_eq!(*writes.lock().unwrap(), [50, 51]);
    }
}

#[test]
fn partition_refuses_unrepresentable_disk_geometry() {
    for (sector, sectors) in [(768, 100), (4096, (u64::MAX / 4096) + 1)] {
        let disk = Disk {
            sector, sectors, table: Vec::new(), writes: Arc::new(Mutex::new(Vec::new())),
        };
        assert!(Partition::new(Box::new(disk), 8, 12).is_err());
    }
}
