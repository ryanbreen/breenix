use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::ffi::c_void;
use std::mem::{offset_of, size_of};
use std::ptr::{self, NonNull};
use std::sync::Mutex;

use arm64_boot_contract::{hw_config::HardwareConfig, phys_memory::PhysRange};
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

const STRIDE: usize = 48;
const BASE: u64 = 0x6000_0000;
const RANGE: u64 = 0x20_0000;

#[derive(Default)]
struct Firmware {
    key: usize,
    exits: usize,
    pool: usize,
    layout: Option<Layout>,
    pool_type: u32,
}
static FIRMWARE: Mutex<Firmware> = Mutex::new(Firmware {
    key: 0,
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
    *stride = STRIDE;
    *version = 1;
    *size = (82 + usize::from(firmware.exits == 1)) * STRIDE;
    if buffer.is_null() || capacity < *size {
        return Status::BUFFER_TOO_SMALL;
    }
    firmware.key += 1;
    *key = firmware.key;
    let mut records =
        std::slice::from_raw_parts_mut(buffer.cast::<u8>(), *size).chunks_exact_mut(STRIDE);
    for range in expected() {
        // The early and stale maps describe different conventional RAM.
        let start = range.start + if firmware.exits == 0 { RANGE * 160 } else { 0 };
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

#[test]
fn final_48_byte_map_keeps_all_40_exact_intervals() {
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

    let mut config = HardwareConfig::new();
    unsafe {
        uefi::table::set_system_table(&table);
        uefi::boot::set_image_handle(
            uefi::Handle::from_ptr(NonNull::<c_void>::dangling().as_ptr()).unwrap(),
        );
        handoff::populate_ram_regions(&mut config);
        handoff::exit_and_encode(&mut config);
    }
    let firmware = FIRMWARE.lock().unwrap();
    assert_eq!(firmware.exits, 2);
    let ranges =
        boot_ranges::from_hardware_config(&config, PhysRange::new(BASE, 0x1_0000_0000), []);
    let expected = expected();
    let missing: Vec<_> = expected
        .iter()
        .copied()
        .filter(|r| !ranges.as_slice().contains(r))
        .collect();
    assert_eq!(
        missing,
        [],
        "firmware RAM intervals missing from kernel pool"
    );
    assert_eq!(ranges.as_slice(), expected);
}
