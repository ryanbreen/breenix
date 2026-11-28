use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::mem::{offset_of, size_of};
use std::ptr::{self, NonNull};
use std::sync::Mutex;

use arm64_boot_contract::{
    hw_config::{HardwareConfig, RamRegion, UefiMapHandoff},
    phys_memory::{boot_ips, boot_pa_limit, PhysRange, DIRECT_MAP_RAM_END},
    uefi_map::{self, MetaError, BOOT_MAP_MEMORY_TYPE},
};
use boot_ranges::{BootMemory, MapError, ProbeError, RamWindow, RangeError, RangeStorage};
use uefi_raw::table::{
    boot::{BootServices, MemoryDescriptor, MemoryType},
    runtime::RuntimeServices,
    system::SystemTable,
    Header,
};
use uefi_raw::{Handle, Status};

#[path = "../../../kernel/src/memory/arm64_direct_map.rs"]
mod arm64_direct_map;

// `RangeError::AboveAddressLimit` is the metadata builder's, which this test
// does not include.
#[allow(dead_code)]
#[path = "../../../kernel/src/memory/boot_ranges.rs"]
mod boot_ranges;
#[path = "../../../parallels-loader/src/handoff.rs"]
mod handoff;

#[path = "../../../parallels-loader/src/page_tables.rs"]
mod page_tables;

use page_tables::{build_page_tables, LoaderExecution, PageTableConfig, PageTableStorage};

const BASE: u64 = 0x6000_0000;
const RANGE: u64 = 0x20_0000;

#[derive(Default)]
struct Firmware {
    key: usize,
    allocations: usize,
    stride: usize,
    refusal: Option<MetaError>,
    exits: usize,
    pool: usize,
    layout: Option<Layout>,
    pool_type: u32,
}
static FIRMWARE: Mutex<Firmware> = Mutex::new(Firmware {
    key: 0,
    allocations: 0,
    stride: 48,
    refusal: None,
    exits: 0,
    pool: 0,
    layout: None,
    pool_type: 0,
});

fn test_window(start: u64, end: u64) -> RamWindow {
    RamWindow::new(start - 0x4000_0000, end, 1 << 48)
}

fn expected() -> Vec<PhysRange> {
    (0..40)
        .map(|i| PhysRange::new(BASE + 2 * i * RANGE, BASE + (2 * i + 1) * RANGE))
        .collect()
}

fn descriptor(bytes: &mut [u8], kind: u32, start: u64, pages: u64, attributes: u64) {
    bytes.fill(0xa5);
    bytes[..4].copy_from_slice(&kind.to_le_bytes());
    bytes[8..16].copy_from_slice(&start.to_le_bytes());
    bytes[16..24].fill(0);
    bytes[24..32].copy_from_slice(&pages.to_le_bytes());
    bytes[32..40].copy_from_slice(&attributes.to_le_bytes());
}

unsafe extern "efiapi" fn get_memory_map(
    size: *mut usize,
    buffer: *mut MemoryDescriptor,
    key: *mut usize,
    stride: *mut usize,
    version: *mut u32,
) -> Status {
    let mut firmware = FIRMWARE.lock().unwrap();
    let capacity = *size;
    *stride = firmware.stride;
    *version = 1;
    *size = (82 + usize::from(firmware.exits == 1)) * firmware.stride;
    if buffer.is_null() || capacity < *size {
        return Status::BUFFER_TOO_SMALL;
    }
    firmware.key += 1;
    *key = firmware.key;
    if firmware.exits == 1 {
        if let Some(error) = firmware.refusal {
            if error == MetaError::Version {
                *version = 2;
            } else {
                *stride = 44;
                *size = 83 * 44;
            }
            let bytes = std::slice::from_raw_parts_mut(buffer.cast::<u8>(), *size);
            bytes.fill(0);
            descriptor(&mut bytes[..40], 5, 0xa000_0000, 1, 1 << 63);
            return Status::SUCCESS;
        }
    }
    let mut records = std::slice::from_raw_parts_mut(buffer.cast::<u8>(), *size)
        .chunks_exact_mut(firmware.stride);
    for range in expected() {
        // The early and stale maps describe different conventional RAM.
        let start = range.start
            + match (firmware.allocations, firmware.exits) {
                (1, _) => RANGE * 240,
                (_, 0) => RANGE * 160,
                _ => 0,
            };
        descriptor(records.next().unwrap(), 7, start, RANGE / 4096, 0);
        let guard = records.next().unwrap();
        descriptor(guard, 0, start + RANGE, RANGE / 4096, 0);
    }
    descriptor(records.next().unwrap(), 5, 0xa000_0000, 1, 1 << 63);
    descriptor(
        records.next().unwrap(),
        firmware.pool_type,
        firmware.pool as u64,
        ((firmware.layout.unwrap().size() + 4095) / 4096) as u64,
        0,
    );
    if let Some(extra) = records.next() {
        descriptor(extra, 4, 0xa000_1000, 1, 0);
    }
    // Scramble in a reproducible order, including the vendor tail.
    let bytes = std::slice::from_raw_parts_mut(buffer.cast::<u8>(), *size);
    let mut seed = 0x6a09_e667_f3bc_c909u64;
    for i in (1..bytes.len() / firmware.stride).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let j = seed as usize % (i + 1);
        if i != j {
            let (left, right) = bytes.split_at_mut(i * firmware.stride);
            left[j * firmware.stride..(j + 1) * firmware.stride]
                .swap_with_slice(&mut right[..firmware.stride]);
        }
    }
    Status::SUCCESS
}

unsafe extern "efiapi" fn allocate_pool(ty: MemoryType, size: usize, out: *mut *mut u8) -> Status {
    let mut firmware = FIRMWARE.lock().unwrap();
    let layout = Layout::from_size_align(size, 4096).unwrap();
    *out = alloc_zeroed(layout);
    assert!(!(*out).is_null());
    firmware.pool = *out as usize;
    firmware.layout = Some(layout);
    firmware.pool_type = ty.0;
    firmware.allocations += 1;
    firmware.key += 1;
    Status::SUCCESS
}

unsafe extern "efiapi" fn free_pool(ptr: *mut u8) -> Status {
    let mut firmware = FIRMWARE.lock().unwrap();
    assert_eq!(ptr as usize, firmware.pool);
    dealloc(ptr, firmware.layout.take().unwrap());
    firmware.key += 1;
    Status::SUCCESS
}

unsafe extern "efiapi" fn exit_boot_services(_: Handle, key: usize) -> Status {
    let mut firmware = FIRMWARE.lock().unwrap();
    assert_eq!(key, firmware.key);
    firmware.exits += 1;
    if firmware.exits == 1 {
        Status::INVALID_PARAMETER
    } else {
        Status::SUCCESS
    }
}

unsafe extern "efiapi" fn unexpected() -> ! {
    panic!("unexpected firmware service")
}

/// The UEFI ABI places a header in front of its function-pointer slots.
/// Unused slots contain a trap; the named services have their exact EFIABI signature.
#[repr(C)]
struct ServiceTable<const N: usize> {
    header: Header,
    slots: [*const c_void; N],
}
impl<const N: usize> ServiceTable<N> {
    fn new() -> Self {
        Self {
            header: Header::default(),
            slots: [unexpected as *const c_void; N],
        }
    }

    fn install(&mut self, offset: usize, service: *const c_void) {
        self.slots[(offset - size_of::<Header>()) / size_of::<usize>()] = service;
    }
}

fn final_map(stride: usize, refusal: Option<MetaError>) {
    let mut boot = ServiceTable::<
        { (size_of::<BootServices>() - size_of::<Header>()) / size_of::<usize>() },
    >::new();
    boot.install(
        offset_of!(BootServices, get_memory_map),
        get_memory_map as *const c_void,
    );
    boot.install(
        offset_of!(BootServices, allocate_pool),
        allocate_pool as *const c_void,
    );
    boot.install(
        offset_of!(BootServices, free_pool),
        free_pool as *const c_void,
    );
    boot.install(
        offset_of!(BootServices, exit_boot_services),
        exit_boot_services as *const c_void,
    );
    let mut runtime = ServiceTable::<
        { (size_of::<RuntimeServices>() - size_of::<Header>()) / size_of::<usize>() },
    >::new();
    let table = SystemTable {
        boot_services: ptr::from_mut(&mut boot).cast(),
        runtime_services: ptr::from_mut(&mut runtime).cast(),
        ..Default::default()
    };

    *FIRMWARE.lock().unwrap() = Firmware {
        stride,
        refusal,
        ..Firmware::default()
    };
    let mut config = HardwareConfig::new();
    unsafe {
        uefi::table::set_system_table(&table);
        uefi::boot::set_image_handle(
            uefi::Handle::from_ptr(NonNull::<c_void>::dangling().as_ptr()).unwrap(),
        );
        assert_eq!(
            handoff::populate_ram_regions().unwrap().0,
            BASE + 240 * RANGE
        );
        handoff::exit_and_encode(&mut config);
    }
    let mut firmware = FIRMWARE.lock().unwrap();
    assert_eq!(firmware.exits, 2);
    assert_eq!(firmware.pool_type, BOOT_MAP_MEMORY_TYPE);
    assert_eq!(config.ram_region_count, 0);
    if let Some(error) = refusal {
        assert_eq!(config.runtime_region_count, 0);
        assert_eq!(config.rtc_get_time, 0);
        assert_eq!(config.uefi_map.loader_check, error as u32);
        let result = boot_ranges::from_hardware_config(
            &config.v7,
            Some(config.uefi_map),
            &Memory::default(),
            test_window(BASE, DIRECT_MAP_RAM_END),
            &[],
        );
        assert_eq!(
            result.err(),
            Some(MapError::Metadata {
                loader_rule: error as u32,
                kernel_rule: Some(error),
            })
        );
        unsafe { dealloc(firmware.pool as *mut u8, firmware.layout.take().unwrap()) };
        return;
    }
    assert_eq!(config.runtime_region_count, 1);
    assert_eq!(config.runtime_regions[0].base, 0xa000_0000);
    assert_eq!(config.uefi_map.loader_check, 0);
    assert_eq!(config.uefi_map.map_size, (83 * stride) as u64);

    // Host virtual addresses exceed the guest's 40-bit IPS. Map the pool to
    // a synthetic IPA for the test walk, while passing its unmodified bytes
    // (including the original buffer pointer) to the production decoder.
    let mut tables = Box::new(PageTableStorage::new());
    let shadow = 0x1_2000_0000;
    let map = PhysRange::from_base_size(shadow, config.uefi_map.map_size);
    let (root, _) = build_page_tables(
        &mut tables,
        &pt_config(0, &[]),
        execution(),
        Some(map),
        boot_ips(2),
    );
    let memory = Memory {
        map_base: firmware.pool as u64,
        map_shadow: shadow,
        root,
        ..Memory::default()
    };
    let (ranges, stats) = boot_ranges::from_hardware_config(
        &config.v7,
        Some(config.uefi_map),
        &memory,
        test_window(BASE, DIRECT_MAP_RAM_END),
        &[],
    )
    .unwrap();
    let mut expected = expected();
    let last = expected[0].end;
    expected[0].end -= 4096;
    assert_eq!(
        memory.pages.borrow().keys().copied().collect::<Vec<_>>(),
        [last - 4096]
    );
    assert_eq!(ranges.iter().collect::<Vec<_>>(), expected);
    assert_eq!(stats.descriptors, 83);
    assert_eq!(stats.stride, stride);
    assert_eq!(stats.usable_bytes, 40 * RANGE);
    assert_eq!(stats.buffer_window, stats.buffer_bytes);
    unsafe { dealloc(firmware.pool as *mut u8, firmware.layout.take().unwrap()) };
}

#[test]
fn stale_exit_key_uses_the_retry_map_at_both_descriptor_strides() {
    final_map(48, None);
    final_map(40, None);
    final_map(48, Some(MetaError::Version));
    final_map(48, Some(MetaError::Stride));
}

#[repr(align(4096))]
struct Page([u8; 4096]);

#[derive(Default)]
struct Memory {
    pages: RefCell<BTreeMap<u64, Box<Page>>>,
    map_base: u64,
    map_shadow: u64,
    root: u64,
    owned_buffer: RefCell<Option<Box<Page>>>,
    failure: Option<ProbeError>,
    storage_ceiling: Option<u64>,
}

unsafe impl RangeStorage for Memory {
    fn storage_page(&self, phys: u64) -> Option<NonNull<u8>> {
        if let Some(limit) = self.storage_ceiling {
            assert!(
                phys < limit,
                "write through an unmapped bootstrap page {phys:#x}"
            );
        }
        let mut pages = self.pages.borrow_mut();
        let page = pages
            .entry(phys)
            .or_insert_with(|| Box::new(Page([0; 4096])));
        NonNull::new(page.0.as_mut_ptr())
    }
}

unsafe impl BootMemory for Memory {
    fn probe_identity_write(&self, page: u64) -> Result<(), ProbeError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.root == 0 {
            return Ok(());
        }
        let shadow = self.map_shadow + page - self.map_base;
        let (pa, flags) = walk(self.root, shadow).ok_or(ProbeError::NoTranslation)?;
        if flags & (1 << 7) != 0 {
            return Err(ProbeError::ReadOnly);
        }
        if pa != shadow {
            return Err(ProbeError::WrongPhysicalAddress);
        }
        if flags & (7 << 2) != 1 << 2 {
            return Err(ProbeError::WrongMemoryType);
        }
        Ok(())
    }

    unsafe fn map_buffer(&self, range: PhysRange) -> NonNull<u8> {
        if let Some(page) = self.owned_buffer.borrow_mut().as_mut() {
            NonNull::new(page.0.as_mut_ptr()).unwrap()
        } else {
            NonNull::new(range.start as *mut u8).unwrap()
        }
    }
}

fn walk(mut table: u64, address: u64) -> Option<(u64, u64)> {
    const OUTPUT: u64 = 0x000f_ffff_ffff_f000;
    for shift in [39, 30, 21, 12] {
        let index = ((address >> shift) & 511) as usize;
        let entry = unsafe { (table as *const u64).add(index).read() };
        if entry & 1 == 0 {
            return None;
        }
        if shift == 12 || entry & 3 == 1 {
            return Some((
                (entry & OUTPUT) + (address & ((1 << shift) - 1)),
                entry & !OUTPUT & !2,
            ));
        }
        table = entry & OUTPUT;
    }
    None
}

fn execution() -> LoaderExecution {
    LoaderExecution {
        here: 0xc001_0000,
        handoff: 0xc001_1000,
        vectors: 0xc001_2000,
        image: PhysRange::new(0xc001_0000, 0xc002_0000),
    }
}

fn pt_config(offset: u64, segments: &[page_tables::KernelLoadSegment]) -> PageTableConfig<'_> {
    PageTableConfig {
        ram_base_offset: offset,
        ecam_ipa: 0,
        ecam_size: 0,
        fb_ipa: 0,
        gicd_ipa: 0,
        gicr_ipa: 0,
        gicr_size: 0,
        kernel_segments: segments,
        uart_base_phys: 0,
        uart_type: 0,
    }
}

#[test]
fn buffer_mapping_preserves_neighbor_and_loader_permissions() {
    const XN: u64 = (1 << 53) | (1 << 54);
    for (address, offset, nx) in [
        (0x4420_4000, 0, true),
        (0xc003_0000, 0, false),
        (0x1_2200_0000, 0, true),
        (0x80_2000_0000, 0, true),
        (0x8420_4000, 0x4000_0000, true),
    ] {
        let segment = page_tables::KernelLoadSegment {
            phys_start: 0x4420_0000 + offset,
            phys_end: 0x4420_4000 + offset,
            flags: 0,
        };
        let mut before = Box::new(PageTableStorage::new());
        let mut after = Box::new(PageTableStorage::new());
        let segments = [segment];
        let config = pt_config(offset, &segments);
        let (old0, old1) = build_page_tables(&mut before, &config, execution(), None, boot_ips(2));
        let range = PhysRange::from_base_size(address + 8, 8192);
        let (new0, new1) =
            build_page_tables(&mut after, &config, execution(), Some(range), boot_ips(5));
        for page in [address, address + 4096, address + 8192] {
            let (physical, flags) = walk(new0, page)
                .unwrap_or_else(|| panic!("map buffer page {page:#x} is unmapped in TTBR0"));
            assert_eq!(physical, page);
            assert_eq!(flags & (1 << 7), 0, "EL1 write denied at {page:#x}");
            assert_eq!(flags & (7 << 2), 1 << 2, "not normal WB");
            assert_eq!(flags & XN, if nx { XN } else { 0 });
            assert_eq!(walk(new1, page), walk(old1, page));
        }
        for page in [
            address - 4096,
            address + 3 * 4096,
            segment.phys_start,
            execution().here,
        ] {
            assert_eq!(walk(new0, page), walk(old0, page), "TTBR0 at {page:#x}");
            assert_eq!(walk(new1, page), walk(old1, page), "TTBR1 at {page:#x}");
        }
    }
}

fn fixture(records: &[(u32, u64, u64, u64)], phys: u64) -> (Memory, UefiMapHandoff) {
    let mut buffer = Box::new(Page([0; 4096]));
    let length = records.len() * 48;
    for (record, &(kind, start, pages, attributes)) in
        buffer.0[..length].chunks_exact_mut(48).zip(records)
    {
        descriptor(record, kind, start, pages, attributes);
    }
    let mut wire = uefi_map::encode_uefi_map(&buffer.0, length, 48, 1);
    wire.map_phys = phys;
    let memory = Memory {
        owned_buffer: RefCell::new(Some(buffer)),
        ..Memory::default()
    };
    (memory, wire)
}

#[test]
fn conventional_ram_excludes_nvs_overlap_and_specific_purpose_memory() {
    let owner = BASE + 2 * RANGE;
    let records = [
        (7, BASE, 32, 0),
        (10, BASE + 0x8000, 1, 0),
        (7, BASE + RANGE, 32, uefi_map::ATTR_SPECIFIC_PURPOSE),
        (BOOT_MAP_MEMORY_TYPE, owner, 1, 0),
    ];
    let (memory, wire) = fixture(&records, owner);
    let (ranges, stats) = boot_ranges::from_hardware_config(
        &HardwareConfig::new().v7,
        Some(wire),
        &memory,
        test_window(BASE, owner),
        &[],
    )
    .unwrap();
    let usable = ranges.iter().collect::<Vec<_>>();
    let wrongly_usable: Vec<_> = [BASE + 0x8000, BASE + RANGE]
        .into_iter()
        .filter(|page| usable.iter().any(|r| (r.start..r.end).contains(page)))
        .collect();
    assert_eq!(wrongly_usable, [], "reserved pages in the frame pool");
    assert_eq!(stats.overlap_bytes, 4096);
    assert_eq!(
        usable,
        [
            PhysRange::new(BASE, BASE + 0x8000),
            // The last page of the larger fragment holds the range set.
            PhysRange::new(BASE + 0x9000, BASE + 0x1f000),
        ]
    );
}

#[test]
fn buffer_release_excludes_fixed_runtime_overlapping_and_unrelated_pages() {
    let base = 0x4300_0000;
    let conventional = 0x7800_0000;
    let owner = base + 0x4000;
    let config = HardwareConfig::new();
    for attributes in [0, 1 << 63, 1 << 18] {
        let records = [
            (BOOT_MAP_MEMORY_TYPE, base, 12, attributes),
            (10, base + 0x6000, 1, 0),
            (BOOT_MAP_MEMORY_TYPE, 0x4900_0000, 4, 0),
            (7, conventional, 32, 0),
            (7, conventional + 0x40_0000, 32, 1 << 18),
            (7, conventional + 0x80_0001, 32, 0),
            (7, conventional + 0x90_0000, 0, 0),
            (7, u64::MAX & !4095, 4, 0),
            (7, conventional + 0x10_0000, 512 * 1024, 0),
        ];
        let (memory, wire) = fixture(&records, owner);
        let fixed = [PhysRange::new(base, base + 0x5000)];
        let (mut ranges, stats) = boot_ranges::from_hardware_config(
            &config.v7,
            Some(wire),
            &memory,
            test_window(base, DIRECT_MAP_RAM_END),
            &fixed,
        )
        .unwrap();
        // The final broad conventional entry overlaps the SP and malformed
        // reservations. All multiply described pages stay out.
        let released = [
            PhysRange::new(base + 0x5000, base + 0x6000),
            PhysRange::new(base + 0x7000, base + 0xc000),
        ];
        let usable = ranges.iter().collect::<Vec<_>>();
        if attributes == 0 {
            assert_eq!(&usable[..2], &released);
            assert_eq!(ranges.carve(0x5000, 4096, base + 0xc000), Ok(released[1]));
            assert_eq!(stats.released_bytes, 0x6000);
            assert_eq!(stats.buffer_fixed, 0x5000);
            assert_eq!(stats.buffer_overlap, 4096);
        } else {
            assert!(usable.iter().all(|range| range.start >= conventional));
            assert_eq!(stats.buffer_attributes, 0xc000);
            assert_eq!(stats.released_bytes, 0);
        }
        assert_eq!(stats.other_map_allocations, 1);
        assert_eq!(stats.malformed, 3);
        assert!(!usable
            .iter()
            .any(|r| (r.start..r.end).contains(&(conventional + 0x40_0000))));
    }
}

#[test]
fn validates_metadata_mapping_and_ownership_before_consuming_descriptors() {
    let phys = 0x6000_0000;
    let config = HardwareConfig::new();
    let window = test_window(phys, DIRECT_MAP_RAM_END);
    let invalid = [
        (
            MetaError::Address,
            UefiMapHandoff {
                map_phys: phys + 1,
                ..Default::default()
            },
        ),
        (
            MetaError::Version,
            UefiMapHandoff {
                map_phys: phys,
                descriptor_version: 2,
                ..Default::default()
            },
        ),
        (
            MetaError::Stride,
            UefiMapHandoff {
                map_phys: phys,
                descriptor_version: 1,
                descriptor_size: 36,
                ..Default::default()
            },
        ),
        (
            MetaError::Stride,
            UefiMapHandoff {
                map_phys: phys,
                descriptor_version: 1,
                descriptor_size: 44,
                ..Default::default()
            },
        ),
        (
            MetaError::Length,
            UefiMapHandoff {
                map_phys: phys,
                descriptor_version: 1,
                descriptor_size: 48,
                map_size: 49,
                ..Default::default()
            },
        ),
    ];
    for (rule, wire) in invalid {
        assert_eq!(
            boot_ranges::from_hardware_config(
                &config.v7,
                Some(wire),
                &Memory::default(),
                window,
                &[]
            )
            .err(),
            Some(MapError::Metadata {
                loader_rule: 0,
                kernel_rule: Some(rule)
            })
        );
    }
    let (mut memory, mut wire) = fixture(&[(7, phys, 512, 0)], phys);
    for error in [
        ProbeError::NoTranslation,
        ProbeError::ReadOnly,
        ProbeError::WrongPhysicalAddress,
        ProbeError::WrongMemoryType,
    ] {
        memory.failure = Some(error);
        assert_eq!(
            boot_ranges::from_hardware_config(&config.v7, Some(wire), &memory, window, &[]).err(),
            Some(MapError::Translation(phys, error))
        );
    }
    memory.failure = None;
    assert_eq!(
        boot_ranges::from_hardware_config(&config.v7, Some(wire), &memory, window, &[]).err(),
        Some(MapError::UnownedBuffer)
    );
    wire.loader_check = MetaError::Length as u32;
    assert_eq!(
        boot_ranges::from_hardware_config(&config.v7, Some(wire), &memory, window, &[]).err(),
        Some(MapError::Metadata {
            loader_rule: 4,
            kernel_rule: None
        })
    );
    let mut buffer = [0u8; 48];
    descriptor(&mut buffer, 5, 0xa000_0000, 1, 1 << 63);
    for (size, stride, error) in [
        (36, 36, MetaError::Stride),
        (49, 48, MetaError::Length),
        (96, 48, MetaError::Length),
    ] {
        let mut config = HardwareConfig::new();
        config.rtc_get_time = 1;
        handoff::encode_final_map(&mut config, &buffer, size, stride, 1);
        assert_eq!(config.uefi_map.loader_check, error as u32);
        assert_eq!(config.runtime_region_count, 0);
        assert_eq!(config.rtc_get_time, 0);
    }
}

#[test]
fn legacy_prefix_only_and_unknown_versions() {
    let mut config = HardwareConfig::new().v7;
    config.version = 7;
    config.ram_region_count = 32;
    for (slot, range) in config.ram_regions.iter_mut().zip(expected()) {
        *slot = RamRegion {
            base: range.start,
            size: range.len(),
        };
    }
    let memory = Memory::default();
    let window = test_window(BASE, DIRECT_MAP_RAM_END);
    let (pool, _) = boot_ranges::from_hardware_config(&config, None, &memory, window, &[]).unwrap();
    let mut want = expected()[..32].to_vec();
    want[0].end -= 4096;
    assert_eq!(pool.iter().collect::<Vec<_>>(), want);
    config.version = 9;
    assert_eq!(
        boot_ranges::from_hardware_config(&config, None, &memory, window, &[]).err(),
        Some(MapError::Version(9))
    );
}

fn fragmented_x86_ranges() -> impl Iterator<Item = PhysRange> + Clone {
    (0..200).rev().map(|i| {
        let base = 0x10_0000 + 0x1_0000 * i;
        PhysRange::new(base, base + 0x8000)
    })
}

#[test]
fn fragmented_x86_ranges_grow_and_carve_the_highest_aligned_fit() {
    let memory = Memory::default();
    let ranges = fragmented_x86_ranges().chain([PhysRange::new(0x8000, 0x9000)]);
    let mut pool = boot_ranges::from_x86(&memory, ranges).unwrap();
    assert_eq!(pool.len(), 200);
    let mut starts: Vec<_> = fragmented_x86_ranges().map(|r| r.start).collect();
    starts.sort_unstable();
    assert!(pool.iter().map(|r| r.start).eq(starts));
    assert_eq!(
        RangeError::NoFit.as_str(),
        "no boot range fits requested carve"
    );
    assert_eq!(memory.pages.borrow().len(), 2);
    assert_eq!(pool.total_bytes(), 200 * 0x8000 - 2 * 4096);
    assert_eq!(
        pool.carve(4096, 0x2000, 0x10_7000),
        Ok(PhysRange::new(0x10_6000, 0x10_7000))
    );
    assert_eq!(pool.carve(0x1_0000, 4096, u64::MAX), Err(RangeError::NoFit));
    assert_eq!(pool.total_bytes(), 200 * 0x8000 - 3 * 4096);
}

fn memory_fdt(ranges: impl IntoIterator<Item = PhysRange>) -> Vec<u8> {
    let mut regs = Vec::new();
    for range in ranges {
        regs.extend(range.start.to_be_bytes());
        regs.extend(range.len().to_be_bytes());
    }

    // A version-17 tree: root default cells are address=2, size=1, so
    // explicitly provide two size cells for the memory node's tuples.
    let names = b"#size-cells\0device_type\0reg\0";
    let mut structure = Vec::new();
    fn push(data: &mut Vec<u8>, word: u32) {
        data.extend(word.to_be_bytes());
    }
    fn prop(data: &mut Vec<u8>, name: u32, value: &[u8]) {
        for word in [3, value.len() as u32, name] {
            push(data, word);
        }
        data.extend(value);
        data.resize(data.len().next_multiple_of(4), 0);
    }
    push(&mut structure, 1);
    push(&mut structure, 0);
    prop(&mut structure, 0, &2u32.to_be_bytes());
    push(&mut structure, 1);
    structure.extend(b"memory@60000000\0");
    prop(&mut structure, 12, b"memory\0");
    prop(&mut structure, 24, &regs);
    for word in [2, 2, 9] {
        push(&mut structure, word);
    }

    let mut blob = Vec::new();
    let strings = 56 + structure.len();
    for word in [
        0xd00d_feed,
        (strings + names.len()) as u32,
        56,
        strings as u32,
        40,
        17,
        16,
        0,
        names.len() as u32,
        structure.len() as u32,
    ] {
        push(&mut blob, word);
    }
    blob.extend([0; 16]); // Empty reservation table.
    blob.extend(structure);
    blob.extend(names);
    blob
}

#[test]
fn fdt_uses_all_memory_tuples_and_stays_inside_the_storage_window() {
    let blob = memory_fdt(
        expected()
            .into_iter()
            .chain([PhysRange::new(0x2_0000_0000, 0x2_8000_0000)]),
    );
    let memory = Memory::default();
    let (ranges, _) = boot_ranges::from_fdt(
        &memory,
        &blob,
        0x4000_0000,
        &[],
        test_window(BASE, DIRECT_MAP_RAM_END),
    )
    .unwrap();
    let mut expected = expected();
    assert_eq!(ranges.len(), expected.len());
    expected[0].end -= 4096;
    assert_eq!(ranges.iter().collect::<Vec<_>>(), expected);
    assert!(memory
        .pages
        .borrow()
        .keys()
        .all(|address| *address < 0x1_0000_0000));
}

#[test]
fn heap_and_stack_carves_remove_their_runs_from_the_pool() {
    let memory = Memory::default();
    let mut pool = boot_ranges::from_x86(
        &memory,
        [PhysRange::new(0x4000_0000, 0x6000_0000)].into_iter(),
    )
    .unwrap();
    let limit = 0x5f00_0000;
    let initial = pool.total_bytes();
    let pools = boot_ranges::place_kernel_pools(&mut pool, 32 << 20, limit).unwrap();
    assert_eq!(pools.heap, PhysRange::new(limit - (64 << 20), limit));
    assert_eq!(
        pools.stacks,
        PhysRange::new(limit - (96 << 20), limit - (64 << 20))
    );
    assert_eq!(initial - pool.total_bytes(), 96 << 20);
    assert!(pool
        .iter()
        .all(|range| range.end <= pools.stacks.start || range.start >= pools.heap.end));
}

#[test]
fn e1_2_device_tree_extends_six_gib_and_counts_unaddressable_ram() {
    let base = 0x4000_0000;
    let end = 0x1_c000_0000;
    let high = 2 << 40;
    let outside = 1 << 48;
    let fixed = [PhysRange::new(base, base + 0x400_0000)];
    let blob = memory_fdt([
        PhysRange::new(base, end),
        PhysRange::new(high, high + 0x2000),
        PhysRange::new(outside - 0x1000, outside + 0x3000),
    ]);
    let memory = Memory::default();
    let (ranges, stats) = boot_ranges::from_fdt(
        &memory,
        &blob,
        base,
        &fixed,
        test_window(base, boot_pa_limit(boot_ips(5))),
    )
    .unwrap();
    assert_eq!(stats.excluded_pa_bytes, 0x3000);
    assert_eq!(stats.excluded_va_bytes, 0);
    assert_eq!(
        ranges.iter().collect::<Vec<_>>(),
        [
            PhysRange::new(fixed[0].end, DIRECT_MAP_RAM_END - 0x1000),
            PhysRange::new(DIRECT_MAP_RAM_END, end),
            PhysRange::new(high, high + 0x2000),
            PhysRange::new(outside - 0x1000, outside),
        ],
    );
    for (parange, width) in [32, 36, 40, 42, 44, 48, 48, 48].into_iter().enumerate() {
        let limit = 1 << width;
        let edge = PhysRange::new(limit - 0x1000, limit + 0x3000);
        let low = PhysRange::new(base + 0x400_0000, base + 0x800_0000);
        let blob = memory_fdt([low, edge]);
        let memory = Memory::default();
        let (ranges, stats) = boot_ranges::from_fdt(
            &memory,
            &blob,
            base,
            &[],
            test_window(base, boot_pa_limit(boot_ips(parange as u64))),
        )
        .unwrap();
        assert_eq!(
            ranges.iter().last(),
            Some(PhysRange::new(edge.start, limit))
        );
        assert_eq!(stats.excluded_pa_bytes, 0x3000);
    }
}

#[test]
fn e1_2_uefi_keeps_high_ram_and_releases_high_buffer() {
    let base = 0x4000_0000;
    let end = 0x1_c000_0000;
    let buffer = end - RANGE;
    let mut descriptors = vec![(7, base, (DIRECT_MAP_RAM_END - base) / 4096, 0)];
    let mut expected = vec![PhysRange::new(base, DIRECT_MAP_RAM_END - 4096)];

    // The conventional regions span 6 GiB with a reserved page between
    // each high range. The last range joins the released UEFI owner.
    for index in 0..39 {
        let start = (DIRECT_MAP_RAM_END + (buffer - DIRECT_MAP_RAM_END) * index / 39) & !4095;
        let limit = (DIRECT_MAP_RAM_END + (buffer - DIRECT_MAP_RAM_END) * (index + 1) / 39) & !4095;
        let usable_end = if index == 38 { limit } else { limit - 4096 };
        descriptors.push((7, start, (usable_end - start) / 4096, 0));
        if index != 38 {
            descriptors.push((0, usable_end, 1, 0));
        }
        expected.push(PhysRange::new(
            start,
            if index == 38 { end } else { usable_end },
        ));
    }
    descriptors.push((BOOT_MAP_MEMORY_TYPE, buffer, (end - buffer) / 4096, 0));
    descriptors.reverse();
    let (mut memory, wire) = fixture(&descriptors, buffer);
    let mut tables = PageTableStorage::new();
    (memory.root, _) = build_page_tables(
        &mut tables,
        &pt_config(0, &[]),
        execution(),
        Some(PhysRange::new(buffer, buffer + wire.map_size)),
        boot_ips(5),
    );
    memory.map_base = buffer;
    memory.map_shadow = buffer;
    let (ranges, stats) = boot_ranges::from_hardware_config(
        &HardwareConfig::new().v7,
        Some(wire),
        &memory,
        test_window(base, boot_pa_limit(boot_ips(5))),
        &[],
    )
    .unwrap();
    assert_eq!(ranges.iter().collect::<Vec<_>>(), expected);
    assert_eq!(stats.released_bytes, end - buffer);
}

#[test]
fn e1_2_loader_ttbr1_leaves_high_usable_ram_unmapped() {
    let high = 0x1_5000_0000;
    let execution = LoaderExecution {
        here: high,
        handoff: high + 0x1000,
        vectors: high + 0x2000,
        image: PhysRange::new(high, high + 0x20000),
    };
    let mut tables = PageTableStorage::new();
    let (ttbr0, ttbr1) = build_page_tables(
        &mut tables,
        &pt_config(0, &[]),
        execution,
        None,
        boot_ips(5),
    );
    assert_eq!(walk(ttbr0, high).unwrap().0, high);
    assert_eq!(walk(ttbr1, high + 0x20_0000), None);
    let storage = &tables as *const _ as u64;
    assert_eq!(walk(ttbr1, storage).unwrap().0, storage);
    assert_eq!(walk(ttbr1, storage - 4096), None);
}

#[test]
fn e1_2_range_storage_grows_below_the_already_mapped_limit() {
    let high = PhysRange::new(DIRECT_MAP_RAM_END, 0x1_c000_0000);
    let low = (0..180).map(|i| {
        let start = 0x6000_0000 + i * 0x8000;
        PhysRange::new(start, start + 0x4000)
    });
    let blob = memory_fdt(low.chain([high]));
    let memory = Memory {
        storage_ceiling: Some(DIRECT_MAP_RAM_END),
        ..Memory::default()
    };
    let (pool, _) = boot_ranges::from_fdt(
        &memory,
        &blob,
        0x4000_0000,
        &[],
        test_window(0x4000_0000, boot_pa_limit(boot_ips(5))),
    )
    .unwrap();
    assert_eq!(pool.iter().last(), Some(high));
    let storage = memory.pages.borrow();
    assert!(storage.len() >= 2);
    assert!(storage.keys().all(|page| *page < DIRECT_MAP_RAM_END));
}

#[test]
fn e1_2_parange_controls_the_loader_buffer_mapping() {
    let address = 2 << 40;
    let mut tables = PageTableStorage::new();
    let range = PhysRange::new(address, address + 0x3000);
    let (ttbr0, ttbr1) = build_page_tables(
        &mut tables,
        &pt_config(0, &[]),
        execution(),
        Some(range),
        boot_ips(5),
    );
    assert_eq!(
        walk(ttbr0, address + 0x2000).map(|x| x.0),
        Some(address + 0x2000)
    );
    assert_eq!(walk(ttbr1, address), None);
}

impl arm64_direct_map::TableMemory for Memory {
    fn read(&self, table: u64, index: usize) -> u64 {
        let page = self.storage_page(table).unwrap();
        unsafe { page.as_ptr().cast::<u64>().add(index).read() }
    }

    fn write(&self, table: u64, index: usize, value: u64) {
        let page = self.storage_page(table).unwrap();
        unsafe { page.as_ptr().cast::<u64>().add(index).write(value) }
    }

    fn clear(&self, table: u64) {
        let page = self.storage_page(table).unwrap();
        unsafe { page.as_ptr().write_bytes(0, 4096) }
    }

    fn publish(&self) {}
}

#[test]
fn e1_2_tables_cover_only_ram_with_minimal_leaves_or_leave_the_low_pool() {
    use arm64_direct_map::{ExtensionError, TableMemory, OUTPUT_ADDRESS};

    const ROOT: u64 = 0x1000;
    const LOW: u64 = 0x6000_0000;
    const GIB: u64 = 1 << 30;
    const MIB2: u64 = 1 << 21;
    const PAGE: u64 = 4096;
    let high = 4 * GIB;
    let described = [
        PhysRange::new(high, high + GIB),
        PhysRange::new(high + GIB + PAGE, high + GIB + 3 * MIB2 + PAGE),
        PhysRange::new(2 << 40, (2 << 40) + PAGE),
    ];
    // A loader already uses the middle range's L2 for an unrelated
    // reserved page. The middle range needs two L3s. The 2 TiB sample
    // needs its own L1 plus L2 and L3.
    for low_pages in [4, 8] {
        let memory = Memory {
            storage_ceiling: Some(high),
            ..Memory::default()
        };
        memory.write(ROOT, 0, 0x2000 | 3);
        memory.write(0x2000, 5, 0x3000 | 3);
        memory.write(0x3000, 511, 0x4000 | 3);
        let loader_page = high + 2 * GIB - MIB2;
        let loader_descriptor = loader_page | 0x707;
        memory.write(0x4000, 0, loader_descriptor);
        let ranges = [PhysRange::new(LOW, LOW + low_pages * PAGE)]
            .into_iter()
            .chain(described);
        let blob = memory_fdt(ranges);
        let (mut pool, _) = boot_ranges::from_fdt(
            &memory,
            &blob,
            0x4000_0000,
            &[],
            test_window(0x4000_0000, 1 << 48),
        )
        .unwrap();
        let low_before = pool.iter().next().unwrap();
        let result = arm64_direct_map::extend(&mut pool, &memory, &memory, ROOT);
        if low_pages == 4 {
            assert!(matches!(
                result,
                Err(ExtensionError::TableShortfall {
                    needed: 5,
                    available: 3
                })
            ));
            assert_eq!(pool.iter().collect::<Vec<_>>(), [low_before]);
            assert_eq!(memory.read(0x2000, 4), 0);
            assert_eq!(memory.read(ROOT, 4), 0);
            continue;
        }
        let extension = result.unwrap();
        assert_eq!(extension.table_pages, 5);
        assert_eq!(
            extension.high_bytes,
            described.iter().map(|range| range.len()).sum::<u64>()
        );
        assert_eq!(pool.iter().next().unwrap().len(), 2 * PAGE);
        assert!(memory.pages.borrow().keys().all(|page| *page < high));

        let gb_leaf = memory.read(0x2000, 4);
        assert_eq!(gb_leaf & 3, 1);
        assert_eq!(gb_leaf & OUTPUT_ADDRESS, high);
        let l2 = memory.read(0x2000, 5) & OUTPUT_ADDRESS;
        for block in 1..3 {
            let leaf = memory.read(l2, block);
            assert_eq!(leaf & 3, 1);
            assert_eq!(leaf & OUTPUT_ADDRESS, high + GIB + block as u64 * MIB2);
        }
        let first_l3 = memory.read(l2, 0) & OUTPUT_ADDRESS;
        assert_eq!(memory.read(first_l3, 0), 0);
        for page in 1..512 {
            let leaf = memory.read(first_l3, page);
            assert_eq!(leaf & 3, 3);
            assert_eq!(leaf & OUTPUT_ADDRESS, high + GIB + page as u64 * PAGE);
        }
        let last_l3 = memory.read(l2, 3) & OUTPUT_ADDRESS;
        assert_eq!(
            memory.read(last_l3, 0) & OUTPUT_ADDRESS,
            high + GIB + 3 * MIB2
        );
        assert_eq!(memory.read(last_l3, 1), 0);
        assert_eq!(memory.read(l2, 4), 0);
        assert_eq!(memory.read(0x4000, 0), loader_descriptor);
        let high_l1 = memory.read(ROOT, 4) & OUTPUT_ADDRESS;
        let high_l2 = memory.read(high_l1, 0) & OUTPUT_ADDRESS;
        let high_l3 = memory.read(high_l2, 0) & OUTPUT_ADDRESS;
        assert_eq!(memory.read(high_l3, 0) & OUTPUT_ADDRESS, 2 << 40);
        assert_eq!(memory.read(high_l3, 1), 0);
    }
}
