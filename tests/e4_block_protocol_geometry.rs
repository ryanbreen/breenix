//! Run with rustc --edition=2021 --test tests/e4_block_protocol_geometry.rs.
#![allow(dead_code)]
#[path = "../kernel/src/drivers/ahci/geometry.rs"]
mod ata;
#[path = "../kernel/src/drivers/nvme/identify.rs"]
mod nvme;
#[path = "../kernel/src/drivers/virtio/blk_queue.rs"]
mod blk_queue;
#[path = "../kernel/src/drivers/virtio/blk_config.rs"]
mod virtio;
#[path = "../tools/virtio-blk-host-tests/src/device.rs"]
mod device;

#[test]
fn ata_uses_identify_logical_sectors_and_validity_bits() {
    let mut page = [0u8; 512];
    let word = |page: &mut [u8], index: usize, value: u16| {
        page[index * 2..index * 2 + 2].copy_from_slice(&value.to_le_bytes());
    };
    page[20..24].copy_from_slice(b"12AB");
    word(&mut page, 83, 0x6400); // Valid, LBA48 and FLUSH CACHE EXT.
    word(&mut page, 100, 128);
    word(&mut page, 106, 0x6003);
    let disk = ata::parse(&page).unwrap();
    assert_eq!((disk.sectors, disk.logical_bytes, disk.physical_bytes), (128, 512, 4096));
    word(&mut page, 106, 0x5000);
    word(&mut page, 117, 2048);
    let disk = ata::parse(&page).unwrap();
    assert_eq!((disk.sectors, disk.logical_bytes, disk.physical_bytes), (128, 4096, 4096));
    assert!(disk.matches_after_reset(ata::parse(&page).unwrap()));
    word(&mut page, 100, 129);
    assert!(!disk.matches_after_reset(ata::parse(&page).unwrap()));
    word(&mut page, 117, 768);
    assert!(ata::parse(&page).is_err());
    word(&mut page, 106, 0xffff); // Unreported: words 117–118 are unusable.
    assert_eq!(ata::parse(&page).unwrap().logical_bytes, 512);
    word(&mut page, 83, 0x2400);
    assert!(ata::parse(&page).is_err());
}

#[test]
fn nvme_active_format_rejects_metadata_protection_and_overflow() {
    let mut page = [0u8; 4096];
    page[..8].copy_from_slice(&512u64.to_le_bytes());
    page[25] = 16; // Seventeen formats, the active one is index 16.
    page[26] = 0x20;
    page[130] = 9;
    page[130 + 4 * 16] = 12;
    let disk = nvme::parse_namespace(&page).unwrap();
    assert_eq!((disk.sectors, disk.logical_bytes), (512, 4096));
    page[29] = 1;
    assert!(nvme::parse_namespace(&page).is_err());
    page[29] = 0;
    page[128 + 4 * 16] = 8;
    assert!(nvme::parse_namespace(&page).is_err());
    page[128 + 4 * 16] = 0;
    page[..8].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(nvme::parse_namespace(&page).is_err());
}

#[test]
fn virtio_reads_one_generation_and_serializes_wire_sectors_with_a_4k_hint() {
    use std::cell::Cell;
    use blk_queue::{request_type, BlkQueue, ChainStorage};

    let reads = Cell::new(0);
    let config = virtio::read_config(
        virtio::BLK_SIZE | virtio::SIZE_MAX | virtio::SEG_MAX | virtio::VERSION_1,
        || { let n = reads.get(); reads.set(n + 1); Some(n.min(1)) },
        |_| unreachable!(),
        |_| unreachable!(),
        |offset| match offset {
            0 => if reads.get() == 1 { 7 } else { 128 },
            4 => 0,
            8 => 4096,
            12 => 1,
            20 => 4096,
            _ => unreachable!(),
        },
    ).unwrap();
    assert_eq!((config.capacity, config.block_hint), (128, 4096));
    assert_eq!(reads.get(), 4);
    let mem = device::shared(8);
    let mut dev = device::Device::new(&mem, config.capacity as usize);
    let mut dma = device::Dma(mem.clone());
    let mut chain = ChainStorage::EMPTY;
    chain.header = 0x1000;
    chain.status = 0x2000;
    chain.frames[0] = 0x3000;
    chain.frame_count = 1;
    let mut queue = BlkQueue::new(device::Ring(mem), 8, config.limits, &[chain]).unwrap();
    let slot = queue.reserve(1).unwrap();
    queue.submit(&mut dma, slot, request_type::IN, 120, None, 4096).unwrap();
    assert!(queue.publish());
    let request = dev.take().pop().unwrap();
    assert_eq!((request.sector, request.data[0].len), (120, 4096));
}

#[test]
fn virtio_rejects_bad_capacity_and_hints() {
    use virtio::{BLK_SIZE, TOPOLOGY};
    for (capacity, hint, physical_exp, offset) in [
        (0, 512, 0, 0), (u64::MAX, 512, 0, 0), (128, 768, 0, 0), (128, 512, 3, 8),
    ] {
        assert!(virtio::read_config(
            BLK_SIZE | TOPOLOGY,
            || None,
            |at| if at == 24 { physical_exp } else { offset },
            |_| 0,
            |at| match at {
                0 => capacity as u32,
                4 => (capacity >> 32) as u32,
                20 => hint,
                28 => 0,
                _ => unreachable!(),
            },
        ).is_err());
    }
}
