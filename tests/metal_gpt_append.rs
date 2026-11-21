//! Exercise the persistent stub's GPT update on an in-memory disk.
#[path = "../scripts/metal/efi-stub/src/gpt_append.rs"]
mod gpt_append;

use gpt_append::{prepare, Header, Part};

const SECTOR: usize = 512;
const ENTRY: usize = 128;
const ARRAY: usize = 128 * ENTRY;
const LAST: u64 = 255;
const PRIMARY: u64 = 3;
const BACKUP: u64 = 220;
const GUID: [u8; 16] = [42; 16];

// UEFI 2.10 §5.3 CRC32, using the MSB-first polynomial with reflected input and output.
fn crc(bytes: &[u8]) -> u32 {
    let mut state = u32::MAX;
    for byte in bytes {
        state ^= u32::from(byte.reverse_bits()) << 24;
        for _ in 0..8 {
            let high = state >> 31;
            state = (state << 1) ^ (high * 0x04c1_1db7);
        }
    }
    !state.reverse_bits()
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn entry(array: &[u8], number: usize) -> &[u8] {
    &array[(number - 1) * ENTRY..number * ENTRY]
}

fn fixture() -> Vec<u8> {
    let mut array = [0; ARRAY];
    for (index, bytes) in array.chunks_exact_mut(ENTRY).take(4).enumerate() {
        let first = 40 + 20 * index as u64;
        bytes[..16].fill(11);
        bytes[16..32].fill(index as u8 + 1);
        bytes[32..40].copy_from_slice(&first.to_le_bytes());
        bytes[40..48].copy_from_slice(&(first + 19).to_le_bytes());
        bytes[55] = 0x80; // A type-specific attribute.
        bytes[56..58].copy_from_slice(&0x03a9u16.to_le_bytes());
    }
    let mut disk = vec![0xa5; (LAST as usize + 1) * SECTOR];
    for (mine, other, entries) in [(1, LAST, PRIMARY), (LAST, 1, BACKUP)] {
        let mut header = [0; SECTOR];
        header[..8].copy_from_slice(b"EFI PART");
        header[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        header[12..16].copy_from_slice(&92u32.to_le_bytes());
        header[24..32].copy_from_slice(&mine.to_le_bytes());
        header[32..40].copy_from_slice(&other.to_le_bytes());
        header[40..48].copy_from_slice(&40u64.to_le_bytes());
        header[48..56].copy_from_slice(&219u64.to_le_bytes());
        header[56..72].copy_from_slice(&GUID);
        header[72..80].copy_from_slice(&entries.to_le_bytes());
        header[80..84].copy_from_slice(&128u32.to_le_bytes());
        header[84..88].copy_from_slice(&128u32.to_le_bytes());
        header[88..92].copy_from_slice(&crc(&array).to_le_bytes());
        let checksum = crc(&header[..92]);
        header[16..20].copy_from_slice(&checksum.to_le_bytes());
        let at = entries as usize * SECTOR;
        disk[at..at + ARRAY].copy_from_slice(&array);
        let at = mine as usize * SECTOR;
        disk[at..at + SECTOR].copy_from_slice(&header);
    }
    disk
}

fn sector(disk: &[u8], lba: u64) -> &[u8] {
    &disk[lba as usize * SECTOR..(lba as usize + 1) * SECTOR]
}

fn array(disk: &[u8], lba: u64) -> &[u8] {
    &disk[lba as usize * SECTOR..lba as usize * SECTOR + ARRAY]
}

fn read_copy(disk: &[u8], lba: u64) -> Option<&[u8]> {
    let header = sector(disk, lba);
    if &header[..8] != b"EFI PART" || u64_at(header, 24) != lba {
        return None;
    }
    let size = u32_at(header, 12) as usize;
    let mut header_for_crc = header.get(..size)?.to_vec();
    header_for_crc.get_mut(16..20)?.fill(0);
    if crc(&header_for_crc) != u32_at(header, 16) {
        return None;
    }
    let bytes = u32_at(header, 80) as usize * u32_at(header, 84) as usize;
    let offset = u64_at(header, 72) as usize * SECTOR;
    let entries = disk.get(offset..offset + bytes)?;
    (crc(entries) == u32_at(header, 88)).then_some(entries)
}

fn additions() -> [Part; 2] {
    let mut name = [0; 36];
    name[..7].copy_from_slice(b"BreenFS");
    [
        Part {
            type_guid: [12; 16],
            unique: [5; 16],
            first: 120,
            last: 159,
            name,
        },
        Part {
            type_guid: [13; 16],
            unique: [6; 16],
            first: 160,
            last: 219,
            name,
        },
    ]
}

fn prepare_fixture<'a>(
    disk: &[u8],
    entries: &'a mut [u8],
    expected: [u8; 16],
    parts: [Part; 2],
) -> Result<gpt_append::Append<'a>, &'static str> {
    prepare(
        Header::parse(sector(disk, 1), 1, LAST)?,
        Header::parse(sector(disk, LAST), LAST, LAST)?,
        entries,
        array(disk, BACKUP),
        expected,
        parts,
    )
}

#[test]
fn refuses_wrong_identity_or_invalid_gpt_copies() {
    let disk = fixture();
    let mut entries = array(&disk, PRIMARY).to_vec();
    assert!(prepare_fixture(&disk, &mut entries, [41; 16], additions()).is_err());
    for (lba, offset) in [(1, 16), (LAST, 16), (PRIMARY, 56), (BACKUP, 56)] {
        let mut damaged = disk.clone();
        damaged[lba as usize * SECTOR + offset] ^= 1;
        let mut entries = array(&damaged, PRIMARY).to_vec();
        assert!(
            prepare_fixture(&damaged, &mut entries, GUID, additions()).is_err(),
            "accepted damaged GPT at LBA {lba}"
        );
    }
}

#[test]
fn refuses_overlaps_out_of_range_and_repeated_guids() {
    let disk = fixture();
    let mut entries = array(&disk, PRIMARY).to_vec();
    for invalid in [
        |parts: &mut [Part; 2]| parts[0].first = 119, // Overlaps entry 4.
        |parts: &mut [Part; 2]| parts[1].first = 159, // Overlaps entry 5.
        |parts: &mut [Part; 2]| parts[1].last = BACKUP,
        |parts: &mut [Part; 2]| parts[1].unique = parts[0].unique,
        |parts: &mut [Part; 2]| parts[0].unique = [3; 16], // Existing partition.
        |parts: &mut [Part; 2]| parts[0].unique = GUID,
    ] {
        let mut parts = additions();
        invalid(&mut parts);
        assert!(prepare_fixture(&disk, &mut entries, GUID, parts).is_err());
        assert_eq!(entries, array(&disk, PRIMARY));
    }
}

#[test]
fn retries_the_same_partition_pair_without_rewriting_gpt() {
    let mut disk = fixture();
    let mut write = |disk: &mut Vec<u8>, lba: u64, bytes: &[u8]| -> Result<(), ()> {
        let start = lba as usize * SECTOR;
        disk[start..start + bytes.len()].copy_from_slice(bytes);
        Ok(())
    };
    let mut entries = array(&disk, PRIMARY).to_vec();
    prepare_fixture(&disk, &mut entries, GUID, additions())
        .unwrap()
        .write(&mut disk, |_, _| Ok(()), &mut write)
        .unwrap();

    let mut foreign = additions();
    foreign[1].unique = [99; 16];
    assert!(prepare_fixture(&disk, &mut entries, GUID, foreign).is_err());
    let retry = prepare_fixture(&disk, &mut entries, GUID, additions()).unwrap();
    let mut initialized = false;
    retry
        .write(
            &mut disk,
            |_, _| {
                initialized = true;
                Ok::<_, ()>(())
            },
            &mut |_, lba, _| panic!("retry rewrote GPT at LBA {lba}"),
        )
        .unwrap();
    assert!(initialized);
}

#[test]
fn every_sector_write_prefix_leaves_a_readable_old_or_complete_new_table() {
    // Distinct payloads in both images, including a multi-sector write and a cleared sector.
    let images = [
        (120, vec![0x51; 2 * SECTOR]),
        (159, vec![0; SECTOR]),
        (160, vec![0x61; SECTOR]),
        (219, vec![0x62; SECTOR]),
    ];
    let old = fixture();
    let mut entries = array(&old, PRIMARY).to_vec();
    let update = prepare_fixture(&old, &mut entries, GUID, additions()).unwrap();
    let mut disk = old.clone();
    let mut writes = Vec::new();
    let mut write = |disk: &mut Vec<u8>, start, bytes: &[u8]| -> Result<(), ()> {
        writes.push((start, bytes.len()));
        for (offset, bytes) in bytes.chunks_exact(SECTOR).enumerate() {
            let at = (start as usize + offset) * SECTOR;
            disk[at..at + SECTOR].copy_from_slice(bytes);

            let mut readable = false;
            for lba in [1, LAST] {
                let Some(entries) = read_copy(disk, lba) else {
                    continue;
                };
                readable = true;
                assert_eq!(&entries[..4 * ENTRY], &array(&old, PRIMARY)[..4 * ENTRY]);
                if entry(entries, 5).iter().all(|&byte| byte == 0) {
                    assert_eq!(entries, array(&old, PRIMARY));
                } else {
                    for (index, part) in additions().iter().enumerate() {
                        let bytes = entry(entries, index + 5);
                        assert_eq!(bytes[..16], part.type_guid);
                        assert_eq!(bytes[16..32], part.unique);
                        assert_eq!(u64_at(bytes, 32), part.first);
                        assert_eq!(u64_at(bytes, 40), part.last);
                    }
                    for (lba, image) in &images {
                        let at = *lba as usize * SECTOR;
                        assert_eq!(&disk[at..at + image.len()], image);
                    }
                }
            }
            assert!(
                readable,
                "no valid GPT after writing LBA {}",
                start as usize + offset
            );
        }
        Ok(())
    };
    update
        .write(
            &mut disk,
            |disk, write| {
                for (lba, bytes) in &images {
                    write(disk, *lba, bytes)?;
                }
                Ok(())
            },
            &mut write,
        )
        .unwrap();
    let metadata = [
        (BACKUP, ARRAY),
        (LAST, SECTOR),
        (PRIMARY, ARRAY),
        (1, SECTOR),
    ];
    assert_eq!(&writes[images.len()..], &metadata);
    let primary = read_copy(&disk, 1).expect("primary header and entry-array CRCs");
    let backup = read_copy(&disk, LAST).expect("backup header and entry-array CRCs");
    assert_eq!(primary, backup);
    assert_eq!(primary, entries);
    assert_eq!(entry(primary, 5)[16..32], [5; 16]);
    assert_eq!(entry(primary, 6)[16..32], [6; 16]);
    assert_eq!(sector(&disk, 0), sector(&old, 0));
}

fn migrated_fixture() -> (Vec<u8>, [Part; 6]) {
    let parts = core::array::from_fn(|index| {
        let first = 40 + 20 * index as u64;
        Part {
            type_guid: [11 + index as u8; 16],
            unique: [1 + index as u8; 16],
            first,
            last: first + 19,
            name: [b'A' + index as u8; 36],
        }
    });
    let mut disk = fixture();
    for (header, table) in [(1, PRIMARY), (LAST, BACKUP)] {
        let at = table as usize * SECTOR;
        for (part, bytes) in parts.iter().zip(disk[at..at + ARRAY].chunks_exact_mut(ENTRY)) {
            bytes.fill(0);
            part.encode(bytes);
        }
        stamp_copy(&mut disk, header, table);
    }
    (disk, parts)
}

fn stamp_copy(disk: &mut [u8], header: u64, table: u64) {
    let checksum = crc(array(disk, table));
    let at = header as usize * SECTOR;
    disk[at + 88..at + 92].copy_from_slice(&checksum.to_le_bytes());
    disk[at + 16..at + 20].fill(0);
    let checksum = crc(&disk[at..at + 92]);
    disk[at + 16..at + 20].copy_from_slice(&checksum.to_le_bytes());
}

fn reclaim_fixture(disk: &mut [u8], parts: [Part; 6], writes: &mut Vec<u64>) -> Result<(), &'static str> {
    let mut entries = array(disk, PRIMARY).to_vec();
    let update = gpt_append::prepare_reclaim(
        Header::parse(sector(disk, 1), 1, LAST)?,
        Header::parse(sector(disk, LAST), LAST, LAST)?,
        &mut entries,
        array(disk, BACKUP),
        GUID,
        parts,
    )?;
    update.write(
        disk,
        &mut |disk, lba, bytes| {
            writes.push(lba);
            let start = lba as usize * SECTOR;
            disk[start..start + bytes.len()].copy_from_slice(bytes);
            Ok(())
        },
        &mut |disk, lba, expected| {
            let start = lba as usize * SECTOR;
            if &disk[start..start + expected.len()] == expected {
                Ok(())
            } else {
                Err("different readback")
            }
        },
    )
}

#[test]
fn reclaim_changes_only_entries_three_and_four_and_their_crcs() {
    let (old, parts) = migrated_fixture();
    let mut disk = old.clone();
    let mut writes = Vec::new();
    reclaim_fixture(&mut disk, parts, &mut writes).unwrap();
    assert_eq!(writes, [BACKUP, LAST, PRIMARY, 1]);

    let mut expected = old;
    for (header, table) in [(1, PRIMARY), (LAST, BACKUP)] {
        let at = table as usize * SECTOR;
        expected[at + 2 * ENTRY..at + 4 * ENTRY].fill(0);
        stamp_copy(&mut expected, header, table);
        assert!(read_copy(&disk, header).is_some());
    }
    assert_eq!(disk, expected);
}

#[test]
fn reclaim_refuses_invalid_or_unexpected_layouts_before_a_write() {
    let (old, parts) = migrated_fixture();
    for (lba, offset, restamp) in [
        (1, 16, false),                 // Header CRC.
        (BACKUP, 56, false),           // Array CRC.
        (BACKUP, 56, true),            // Valid but different backup.
        (PRIMARY, 5 * ENTRY, true),    // Matching copies with a different sixth entry.
        (PRIMARY, 6 * ENTRY, true),    // Matching copies with a seventh entry.
    ] {
        let mut disk = old.clone();
        disk[lba as usize * SECTOR + offset] ^= 1;
        if restamp && lba == PRIMARY {
            disk[BACKUP as usize * SECTOR + offset] ^= 1;
            stamp_copy(&mut disk, 1, PRIMARY);
        }
        if restamp {
            stamp_copy(&mut disk, LAST, BACKUP);
        }
        let original = disk.clone();
        let mut writes = Vec::new();
        assert!(reclaim_fixture(&mut disk, parts, &mut writes).is_err(), "LBA {lba} offset {offset}");
        assert!(writes.is_empty());
        assert_eq!(disk, original);
    }

    let mut original_layout = fixture();
    let mut writes = Vec::new();
    assert!(reclaim_fixture(&mut original_layout, parts, &mut writes).is_err());
    assert!(writes.is_empty());
}
