use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::mem::{offset_of, size_of};
use std::ptr::{self, NonNull};
use std::sync::Mutex;

use arm64_boot_contract::{
    hw_config::{HardwareConfig, RamRegion, UefiMapHandoff},
    phys_memory::PhysRange,
    uefi_map::{self, MetaError, BOOT_MAP_MEMORY_TYPE},
};
use boot_ranges::{BootMemory, MapError, ProbeError, RangeError, RangeStorage};
use uefi_raw::table::{
    boot::{BootServices, MemoryDescriptor, MemoryType},
    runtime::RuntimeServices,
    system::SystemTable,
    Header,
};
use uefi_raw::{Handle, Status};

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
            PhysRange::new(BASE, 0x1_0000_0000),
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
    let (root, _) = build_page_tables(&mut tables, &pt_config(0, &[]), execution(), Some(map));
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
        PhysRange::new(BASE, 0x1_0000_0000),
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
}

unsafe impl RangeStorage for Memory {
    fn storage_page(&self, phys: u64) -> Option<NonNull<u8>> {
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
        let (old0, old1) = build_page_tables(&mut before, &config, execution(), None);
        let range = PhysRange::from_base_size(address + 8, 8192);
        let (new0, new1) = build_page_tables(&mut after, &config, execution(), Some(range));
        for page in [address, address + 4096, address + 8192] {
            let (physical, flags) = walk(new0, page).unwrap();
            assert_eq!(physical, page);
            assert_eq!(flags & (1 << 7), 0, "EL1 write denied");
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
            PhysRange::new(base, 0x1_0000_0000),
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
    let window = PhysRange::new(phys, 0x1_0000_0000);
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
    let window = PhysRange::new(BASE, 0x1_0000_0000);
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

#[test]
fn fragmented_x86_ranges_grow_and_carve_the_highest_aligned_fit() {
    let memory = Memory::default();
    let ranges = (0..200)
        .rev()
        .map(|i| {
            let base = 0x10_0000 + 0x1_0000 * i;
            PhysRange::new(base, base + 0x8000)
        })
        .chain([PhysRange::new(0x8000, 0x9000)]);
    let mut pool = boot_ranges::from_x86(&memory, ranges).unwrap();
    assert_eq!(pool.len(), 200);
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

#[test]
fn fdt_uses_all_memory_tuples_and_stays_inside_the_storage_window() {
    let mut regs = Vec::new();
    for range in expected() {
        regs.extend(range.start.to_be_bytes());
        regs.extend(range.len().to_be_bytes());
    }
    regs.extend(0x2_0000_0000u64.to_be_bytes());
    regs.extend(0x8000_0000u64.to_be_bytes());

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

    let memory = Memory::default();
    let ranges = boot_ranges::from_fdt(
        &memory,
        &blob,
        0x4000_0000,
        &[],
        PhysRange::new(BASE, 0x1_0000_0000),
    )
    .unwrap();
    let mut expected = expected();
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
