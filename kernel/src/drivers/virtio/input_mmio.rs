//! VirtIO Input driver for ARM64 MMIO transport.
//!
//! This driver handles VirtIO input devices (device ID 18) which provide
//! keyboard, mouse, and other input events. It supports two devices:
//! - Keyboard (virtio-keyboard-device): EV_KEY events for key presses
//! - Tablet (virtio-tablet-device): EV_ABS events for absolute mouse position,
//!   EV_KEY events for mouse buttons
//!
//! The virtio-input device sends events to the guest via a single queue.
//! Events use the Linux evdev format:
//! - type: event type (EV_KEY=1, EV_ABS=3)
//! - code: scancode or axis/button code
//! - value: 1=press, 0=release for keys; position for axes

#![cfg(target_arch = "aarch64")]

use super::input_decode::{self, Frame};
use super::mmio::{
    device_id, VirtioMmioDevice, VIRTIO_MMIO_BASE, VIRTIO_MMIO_COUNT, VIRTIO_MMIO_SIZE,
};
use crate::input::hardware;
use core::sync::atomic::{fence, AtomicBool, AtomicU16, AtomicU32, Ordering};
static KBD_FRAME: spin::Mutex<Frame> = spin::Mutex::new(Frame::new());
static TABLET_FRAME: spin::Mutex<Frame> = spin::Mutex::new(Frame::new());
static KBD_INPUT: spin::Once<hardware::Device> = spin::Once::new();
static TABLET_INPUT: spin::Once<hardware::Device> = spin::Once::new();

/// VirtIO descriptor flags
const DESC_F_WRITE: u16 = 2; // Device writes (vs reads)
#[allow(dead_code)]
const DESC_F_NEXT: u16 = 1; // Descriptor continues via next field

// =============================================================================
// VirtIO Input Event Structure
// =============================================================================

/// VirtIO input event (matches Linux evdev format)
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VirtioInputEvent {
    /// Event type (EV_KEY=1, EV_REL=2, EV_ABS=3, etc.)
    pub event_type: u16,
    /// Event code (scancode for keyboard)
    pub code: u16,
    /// Event value (1=press, 0=release for keys)
    pub value: u32,
}

impl VirtioInputEvent {
    pub const fn empty() -> Self {
        Self {
            event_type: 0,
            code: 0,
            value: 0,
        }
    }
}

/// Event types (Linux evdev)
pub mod event_type {
    pub const EV_SYN: u16 = 0x00; // Synchronization
    pub const EV_KEY: u16 = 0x01; // Key press/release
    pub const EV_REL: u16 = 0x02; // Relative movement (mouse)
    pub const EV_ABS: u16 = 0x03; // Absolute position
}

/// Absolute axis codes
mod abs_code {
    pub const ABS_X: u16 = 0x00;
    pub const ABS_Y: u16 = 0x01;
}

/// Button codes for mouse
mod btn_code {
    pub const BTN_LEFT: u16 = 0x110;
}

// =============================================================================
// Static Memory for VirtIO Queue and Buffers — Keyboard
// =============================================================================

/// Number of event buffers for keyboard (increased from 16 for paste support).
/// Paste generates ~3 events per character (press, release, sync).
/// 64 buffers supports ~21 chars in flight, sufficient for paste throughput.
const NUM_KBD_BUFFERS: usize = 64;

/// Number of event buffers for tablet (mouse position + button events)
const NUM_TABLET_BUFFERS: usize = 16;

/// Size of the event structure
const EVENT_SIZE: usize = core::mem::size_of::<VirtioInputEvent>();

/// Event queue memory (descriptors, available ring, used ring)
/// For VirtIO legacy (v1), the used ring must be page-aligned (4096 bytes).
///
/// Layout for N buffers:
/// - Descriptors: N * 16 bytes
/// - Available ring: 6 + N * 2 bytes
/// - Padding: to 4096 byte boundary
/// - Used ring: 6 + N * 8 bytes
#[repr(C, align(4096))]
struct KbdQueueMemory {
    /// Descriptor table (16 bytes each)
    descriptors: [[u8; 16]; NUM_KBD_BUFFERS], // 1024 bytes
    /// Available ring
    avail_flags: u16,
    avail_idx: u16,
    avail_ring: [u16; NUM_KBD_BUFFERS], // 128 bytes
    avail_used_event: u16,
    /// Padding to align used ring to 4096 for legacy VirtIO
    /// Total so far: 1024 + 4 + 128 + 2 = 1158 bytes
    _pad_to_page: [u8; 4096 - 1158],
    /// Used ring (must be page-aligned for legacy devices)
    used_flags: u16,
    used_idx: u16,
    used_ring: [[u8; 8]; NUM_KBD_BUFFERS],
    used_avail_event: u16,
}

#[repr(C, align(4096))]
struct KbdEventBuffers {
    events: [VirtioInputEvent; NUM_KBD_BUFFERS],
}

// Keyboard static memory
static mut KBD_QUEUE_MEM: KbdQueueMemory = KbdQueueMemory {
    descriptors: [[0; 16]; NUM_KBD_BUFFERS],
    avail_flags: 0,
    avail_idx: 0,
    avail_ring: [0; NUM_KBD_BUFFERS],
    avail_used_event: 0,
    _pad_to_page: [0; 4096 - 1158],
    used_flags: 0,
    used_idx: 0,
    used_ring: [[0; 8]; NUM_KBD_BUFFERS],
    used_avail_event: 0,
};

static mut KBD_EVENT_BUFFERS: KbdEventBuffers = KbdEventBuffers {
    events: [VirtioInputEvent::empty(); NUM_KBD_BUFFERS],
};

// Keyboard device state
static mut KBD_DEVICE_BASE: u64 = 0;
static mut KBD_DEVICE_SLOT: usize = 0;
static KBD_INITIALIZED: AtomicBool = AtomicBool::new(false);
static KBD_LAST_USED_IDX: AtomicU16 = AtomicU16::new(0);

// =============================================================================
// Static Memory for VirtIO Queue and Buffers — Tablet
// =============================================================================

#[repr(C, align(4096))]
struct TabletQueueMemory {
    descriptors: [[u8; 16]; NUM_TABLET_BUFFERS], // 256 bytes
    avail_flags: u16,
    avail_idx: u16,
    avail_ring: [u16; NUM_TABLET_BUFFERS], // 32 bytes
    avail_used_event: u16,
    /// Total so far: 256 + 4 + 32 + 2 = 294 bytes
    _pad_to_page: [u8; 4096 - 294],
    used_flags: u16,
    used_idx: u16,
    used_ring: [[u8; 8]; NUM_TABLET_BUFFERS],
    used_avail_event: u16,
}

#[repr(C, align(4096))]
struct TabletEventBuffers {
    events: [VirtioInputEvent; NUM_TABLET_BUFFERS],
}

// Tablet static memory
static mut TABLET_QUEUE_MEM: TabletQueueMemory = TabletQueueMemory {
    descriptors: [[0; 16]; NUM_TABLET_BUFFERS],
    avail_flags: 0,
    avail_idx: 0,
    avail_ring: [0; NUM_TABLET_BUFFERS],
    avail_used_event: 0,
    _pad_to_page: [0; 4096 - 294],
    used_flags: 0,
    used_idx: 0,
    used_ring: [[0; 8]; NUM_TABLET_BUFFERS],
    used_avail_event: 0,
};

static mut TABLET_EVENT_BUFFERS: TabletEventBuffers = TabletEventBuffers {
    events: [VirtioInputEvent::empty(); NUM_TABLET_BUFFERS],
};

// Tablet device state
static mut TABLET_DEVICE_BASE: u64 = 0;
static mut TABLET_DEVICE_SLOT: usize = 0;
static TABLET_INITIALIZED: AtomicBool = AtomicBool::new(false);
static TABLET_LAST_USED_IDX: AtomicU16 = AtomicU16::new(0);

// =============================================================================
// Mouse State (written by tablet interrupt, read by render thread)
// =============================================================================

/// Tablet absolute position range (0..32767 for virtio-tablet-device)
const TABLET_ABS_MAX: u32 = 32767;

/// Get current screen dimensions from the GPU driver (fallback to 1280x800)
#[inline]
fn screen_dimensions() -> (u32, u32) {
    // The GPU core's screen, once it replaces the MMIO display.
    crate::input::pointer::screen()
        .or_else(super::gpu_mmio::dimensions)
        .unwrap_or((1280, 800))
}

/// Current mouse X position in screen coordinates
static MOUSE_X: AtomicU32 = AtomicU32::new(0);
/// Current mouse Y position in screen coordinates
static MOUSE_Y: AtomicU32 = AtomicU32::new(0);
/// Mouse button state (bit 0 = left button)
static MOUSE_BUTTONS: AtomicU32 = AtomicU32::new(0);

// =============================================================================
// Common Helpers
// =============================================================================

/// Base IRQ for VirtIO MMIO devices on QEMU virt machine
/// IRQ = VIRTIO_IRQ_BASE + slot_number
const VIRTIO_IRQ_BASE: u32 = 48;

#[inline(always)]
fn virt_to_phys(addr: u64) -> u64 {
    addr - crate::memory::physical_memory_offset().as_u64()
}

// =============================================================================
// Descriptor Helpers (generic over queue memory)
// =============================================================================

/// Write a VirtIO descriptor into a queue's descriptor table.
///
/// # Safety
/// Caller must ensure `desc_table` points to valid, writable descriptor memory
/// and `idx` is within the table bounds.
unsafe fn write_descriptor_at(
    desc_table: *mut [u8; 16],
    idx: usize,
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
) {
    let desc = &mut *desc_table.add(idx);
    desc[0..8].copy_from_slice(&addr.to_le_bytes());
    desc[8..12].copy_from_slice(&len.to_le_bytes());
    desc[12..14].copy_from_slice(&flags.to_le_bytes());
    desc[14..16].copy_from_slice(&next.to_le_bytes());
}

// =============================================================================
// Driver Implementation
// =============================================================================

/// VirtIO input config select values (per VirtIO spec 5.8.4).
mod input_cfg {
    /// Query supported event type bitmaps.
    /// subsel = event type (EV_KEY, EV_ABS, etc.)
    /// Returns bitmap of supported codes for that event type.
    pub const EV_BITS: u8 = 0x11;
}

/// Check if a VirtIO input device supports absolute positioning (EV_ABS).
///
/// Queries the VirtIO input config space to check if the device reports
/// any EV_ABS (absolute axis) event codes. Pointing devices (tablet, touchscreen)
/// support EV_ABS; keyboards do not.
///
/// This must be called before `device.init()` — the VirtIO input config is
/// readable at any time regardless of device status.
fn is_pointing_device(device: &VirtioMmioDevice) -> bool {
    let base = device.base();
    unsafe {
        // VirtIO input config layout at MMIO offset 0x100:
        //   byte 0: select (write)
        //   byte 1: subsel (write)
        //   byte 2: size (read)
        let config_select = (base + 0x100) as *mut u8;
        let config_subsel = (base + 0x101) as *mut u8;
        let config_size = (base + 0x102) as *const u8;

        // Query EV_BITS for EV_ABS (event type 3)
        crate::arch_impl::aarch64::mmio::write8(config_select, input_cfg::EV_BITS);
        crate::arch_impl::aarch64::mmio::write8(config_subsel, event_type::EV_ABS as u8);

        // If size > 0, the device reports supported ABS axis codes → it's a pointer
        let size = crate::arch_impl::aarch64::mmio::read8(config_size);
        size > 0
    }
}

/// Initialize the VirtIO input driver (keyboard + tablet).
///
/// Scans all VirtIO MMIO slots for input devices. Uses the VirtIO input
/// config space to distinguish keyboards (no EV_ABS support) from pointing
/// devices like tablets (EV_ABS support).
pub fn init() -> Result<(), &'static str> {
    if KBD_INITIALIZED.load(Ordering::Relaxed) {
        return Ok(());
    }

    crate::serial_println!(
        "[virtio-input] Searching for input devices (ID={})...",
        device_id::INPUT
    );

    for i in 0..VIRTIO_MMIO_COUNT {
        let base = VIRTIO_MMIO_BASE + (i as u64) * VIRTIO_MMIO_SIZE;

        if let Some(mut device) = VirtioMmioDevice::probe(base) {
            let id = device.device_id();
            crate::serial_println!("[virtio-input] Slot {} at {:#x}: device_id={}", i, base, id);

            if id == device_id::INPUT {
                if is_pointing_device(&device) {
                    if !TABLET_INITIALIZED.load(Ordering::Relaxed) {
                        crate::serial_println!(
                            "[virtio-input] Found tablet at {:#x} (slot {}, supports EV_ABS)",
                            base,
                            i
                        );
                        init_tablet_device(&mut device, i)?;
                    }
                } else if !KBD_INITIALIZED.load(Ordering::Relaxed) {
                    crate::serial_println!(
                        "[virtio-input] Found keyboard at {:#x} (slot {}, no EV_ABS)",
                        base,
                        i
                    );
                    init_keyboard_device(&mut device, i)?;
                }
            }
        }
    }

    if KBD_INITIALIZED.load(Ordering::Relaxed) {
        Ok(())
    } else {
        Err("No VirtIO input device found")
    }
}

/// Initialize the keyboard device.
fn init_keyboard_device(device: &mut VirtioMmioDevice, slot: usize) -> Result<(), &'static str> {
    crate::serial_println!(
        "[virtio-input] Keyboard device version: {}",
        device.version()
    );

    init_input_device_queue(
        device,
        (&raw mut KBD_QUEUE_MEM) as *mut u8,
        (&raw const KBD_EVENT_BUFFERS).cast::<VirtioInputEvent>() as u64,
        (&raw mut KBD_QUEUE_MEM).cast::<[u8; 16]>(),
        core::mem::offset_of!(KbdQueueMemory, avail_flags) as u64,
        core::mem::offset_of!(KbdQueueMemory, used_flags) as u64,
        NUM_KBD_BUFFERS,
        &raw mut KBD_DEVICE_BASE,
        None,
    )?;

    unsafe {
        // init_input_device_queue already set KBD_DEVICE_BASE to the virtual MMIO address
        *(&raw mut KBD_DEVICE_SLOT) = slot;
    }

    // Enable IRQ in GIC
    let irq = VIRTIO_IRQ_BASE + slot as u32;
    crate::serial_println!("[virtio-input] Enabling IRQ {} for keyboard", irq);
    use crate::arch_impl::aarch64::gic;
    use crate::arch_impl::traits::InterruptController;
    gic::Gicv2::enable_irq(irq as u8);

    let input = crate::input::stream::register_hardware(hardware::Description {
        capabilities: (1..=255)
            .map(|code| input_abi::Capability {
                kind: 1,
                code,
                minimum: 0,
                maximum: 2,
            })
            .collect(),
        keyboard: Some(crate::input::Device::VirtioKeyboard),
    })?;
    KBD_INPUT.call_once(|| input);
    KBD_INITIALIZED.store(true, Ordering::Release);
    crate::input::control::register(&REPLAY, false);
    crate::serial_println!(
        "[virtio-input] Keyboard initialized ({} buffers)",
        NUM_KBD_BUFFERS
    );
    Ok(())
}

/// Initialize the tablet device.
fn init_tablet_device(device: &mut VirtioMmioDevice, slot: usize) -> Result<(), &'static str> {
    crate::serial_println!("[virtio-input] Tablet device version: {}", device.version());

    init_input_device_queue(
        device,
        (&raw mut TABLET_QUEUE_MEM) as *mut u8,
        (&raw const TABLET_EVENT_BUFFERS).cast::<VirtioInputEvent>() as u64,
        (&raw mut TABLET_QUEUE_MEM).cast::<[u8; 16]>(),
        core::mem::offset_of!(TabletQueueMemory, avail_flags) as u64,
        core::mem::offset_of!(TabletQueueMemory, used_flags) as u64,
        NUM_TABLET_BUFFERS,
        &raw mut TABLET_DEVICE_BASE,
        None,
    )?;

    unsafe {
        // init_input_device_queue already set TABLET_DEVICE_BASE to the virtual MMIO address
        *(&raw mut TABLET_DEVICE_SLOT) = slot;
    }

    // Enable IRQ in GIC
    let irq = VIRTIO_IRQ_BASE + slot as u32;
    crate::serial_println!("[virtio-input] Enabling IRQ {} for tablet", irq);
    use crate::arch_impl::aarch64::gic;
    use crate::arch_impl::traits::InterruptController;
    gic::Gicv2::enable_irq(irq as u8);

    let input = crate::input::stream::register_hardware(hardware::Description {
        capabilities: alloc::vec![
            input_abi::Capability {
                kind: 3,
                code: 0,
                minimum: 0,
                maximum: 32767
            },
            input_abi::Capability {
                kind: 3,
                code: 1,
                minimum: 0,
                maximum: 32767
            },
            input_abi::Capability {
                kind: 1,
                code: 272,
                minimum: 0,
                maximum: 1
            },
            input_abi::Capability {
                kind: 1,
                code: 273,
                minimum: 0,
                maximum: 1
            },
            input_abi::Capability {
                kind: 1,
                code: 274,
                minimum: 0,
                maximum: 1
            },
            input_abi::Capability {
                kind: 2,
                code: 6,
                minimum: i32::MIN,
                maximum: i32::MAX
            },
            input_abi::Capability {
                kind: 2,
                code: 8,
                minimum: i32::MIN,
                maximum: i32::MAX
            },
        ],
        keyboard: None,
    })?;
    TABLET_INPUT.call_once(|| input);
    TABLET_INITIALIZED.store(true, Ordering::Release);
    crate::input::control::register(&REPLAY, true);
    crate::serial_println!(
        "[virtio-input] Tablet initialized ({} buffers)",
        NUM_TABLET_BUFFERS
    );
    Ok(())
}

/// Shared device queue initialization for both keyboard and tablet.
///
/// Sets up the VirtIO queue, posts event buffers, and marks device ready.
fn init_input_device_queue(
    device: &mut VirtioMmioDevice,
    queue_mem_base: *mut u8,
    event_buffers_base_virt: u64,
    desc_table: *mut [u8; 16],
    avail_offset: u64,
    used_offset: u64,
    num_buffers: usize,
    device_base_ptr: *mut u64,
    replay: Option<&[VirtioInputEvent]>,
) -> Result<u32, &'static str> {
    let version = device.version();

    if version == 1 {
        device.set_guest_page_size(4096);
    }

    device.init(0)?;

    device.select_queue(0);
    let queue_size = device.get_queue_num_max();

    if queue_size == 0 {
        return Err("Event queue not available");
    }

    let actual_size = num_buffers.min(queue_size as usize);
    device.set_queue_num(actual_size as u32);
    let size = if num_buffers == NUM_TABLET_BUFFERS { &TABLET_QUEUE_SIZE } else { &KBD_QUEUE_SIZE };
    size.store(actual_size as u16, Ordering::Release);

    let queue_phys = virt_to_phys(queue_mem_base as u64);
    let desc_addr = queue_phys;
    let avail_addr = queue_phys + avail_offset;
    let used_addr = queue_phys + used_offset;

    if version == 1 {
        let pfn = (queue_phys >> 12) as u32;
        device.set_queue_align(4096);
        device.set_queue_pfn(pfn);
    } else {
        device.set_queue_desc(desc_addr);
        device.set_queue_avail(avail_addr);
        device.set_queue_used(used_addr);
        device.set_queue_ready(true);
    }

    // Post event buffers
    let event_base_phys = virt_to_phys(event_buffers_base_virt);
    unsafe {
        for i in 0..actual_size {
            let event_addr = event_base_phys + (i * EVENT_SIZE) as u64;
            write_descriptor_at(
                desc_table,
                i,
                event_addr,
                EVENT_SIZE as u32,
                DESC_F_WRITE,
                0,
            );
        }
    }

    // Write available ring (the avail_ring starts right after avail_flags(2) + avail_idx(2))
    // We need to write directly to the queue memory
    // The avail_ring array is at offset avail_offset + 4
    let posted = unsafe {
        let avail_idx_ptr = (queue_mem_base as u64 + avail_offset + 2) as *mut u16;
        let avail_ring_ptr = (queue_mem_base as u64 + avail_offset + 4) as *mut u16;

        core::ptr::write_volatile(avail_idx_ptr, 0);
        // Replay owns buffers only while reset has returned DMA custody. It
        // consumes the recorded completions through the ordinary adapter and
        // reposts those exact descriptors before DRIVER_OK resumes the device.
        let tablet = num_buffers == NUM_TABLET_BUFFERS;
        let replayed = replay.map_or(0, |events| events.len());
        for i in 0..actual_size {
            if let Some(event) = replay.and_then(|events| events.get(i)) {
                core::ptr::write_volatile(
                    (event_buffers_base_virt as *mut VirtioInputEvent).add(i),
                    *event,
                );
                complete_buffer(tablet, i);
            } else {
                let at = core::ptr::read_volatile(avail_idx_ptr);
                core::ptr::write_volatile(avail_ring_ptr.add(at as usize % actual_size), i as u16);
                core::ptr::write_volatile(avail_idx_ptr, at.wrapping_add(1));
            }
        }
        if replayed > actual_size {
            return Err("Replay exceeds bound input ring");
        }
        fence(Ordering::SeqCst);
        let used_idx = (queue_mem_base as u64 + used_offset + 2) as *const u16;
        let posted = core::ptr::read_volatile(avail_idx_ptr)
            .wrapping_sub(core::ptr::read_volatile(used_idx)) as u32;
        posted
    };

    // Notify device (queue 0)
    // device.base() is already the virtual MMIO address (probe adds phys_offset)
    let virt_base = device.base();
    unsafe {
        *device_base_ptr = virt_base;
        let notify_addr = (virt_base + 0x50) as *mut u32;
        crate::arch_impl::aarch64::mmio::write32(notify_addr, 0);
    }

    device.driver_ok();
    Ok(posted)
}

// =============================================================================
// Keyboard Polling and Interrupt Handling
// =============================================================================

/// Drain actual used descriptors through the completion/rearm adapter.
fn poll_events() {
    poll_completions(false);
}

fn poll_completions(tablet: bool) {
    crate::arch_without_interrupts(|| unsafe {
        let _gate = COMPLETION_GATE.lock();
        let (used_idx, used_ring, last, size, base) = if tablet {
            (
                &raw const TABLET_QUEUE_MEM.used_idx,
                (&raw const TABLET_QUEUE_MEM.used_ring).cast::<[u8; 8]>(),
                &TABLET_LAST_USED_IDX,
                TABLET_QUEUE_SIZE.load(Ordering::Acquire) as usize,
                TABLET_DEVICE_BASE,
            )
        } else {
            (
                &raw const KBD_QUEUE_MEM.used_idx,
                (&raw const KBD_QUEUE_MEM.used_ring).cast::<[u8; 8]>(),
                &KBD_LAST_USED_IDX,
                KBD_QUEUE_SIZE.load(Ordering::Acquire) as usize,
                KBD_DEVICE_BASE,
            )
        };
        let mut at = last.load(Ordering::Relaxed);
        let end = core::ptr::read_volatile(used_idx);
        fence(Ordering::Acquire);
        while at != end {
            let entry = core::ptr::read_volatile(used_ring.add(at as usize % size));
            let descriptor = u32::from_le_bytes(entry[..4].try_into().unwrap()) as usize;
            if descriptor < size {
                complete_buffer(tablet, descriptor);
            }
            at = at.wrapping_add(1);
        }
        last.store(at, Ordering::Release);
        fence(Ordering::Release);
        crate::arch_impl::aarch64::mmio::write32((base + 0x50) as *mut u32, 0);
    });
}

/// The same buffer read, pure decoder, report, wake decision and descriptor
/// repost are used by real used-ring completions and reset-custody replays.
unsafe fn complete_buffer(tablet: bool, descriptor: usize) {
    let (buffer, avail_idx, avail_ring, size) = if tablet {
        (
            (&raw const TABLET_EVENT_BUFFERS.events).cast::<VirtioInputEvent>(),
            &raw mut TABLET_QUEUE_MEM.avail_idx,
            (&raw mut TABLET_QUEUE_MEM.avail_ring).cast::<u16>(),
            TABLET_QUEUE_SIZE.load(Ordering::Acquire) as usize,
        )
    } else {
        (
            (&raw const KBD_EVENT_BUFFERS.events).cast::<VirtioInputEvent>(),
            &raw mut KBD_QUEUE_MEM.avail_idx,
            (&raw mut KBD_QUEUE_MEM.avail_ring).cast::<u16>(),
            KBD_QUEUE_SIZE.load(Ordering::Acquire) as usize,
        )
    };
    let raw = core::ptr::read_volatile(buffer.add(descriptor));
    report_event(tablet, raw);
    if tablet { report_tablet_event(raw); }
    let at = core::ptr::read_volatile(avail_idx);
    core::ptr::write_volatile(avail_ring.add(at as usize % size), descriptor as u16);
    fence(Ordering::Release);
    core::ptr::write_volatile(avail_idx, at.wrapping_add(1));
}

/// Check if the keyboard device is initialized
pub fn is_initialized() -> bool {
    KBD_INITIALIZED.load(Ordering::Acquire)
}

/// Debug: get current keyboard ring state (avail_idx, used_idx, last_seen)
pub fn debug_ring_state() -> (u16, u16, u16) {
    if !KBD_INITIALIZED.load(Ordering::Acquire) {
        return (0, 0, 0);
    }
    unsafe {
        let queue_mem = &raw const KBD_QUEUE_MEM;
        let avail_idx = (*queue_mem).avail_idx;
        let used_idx = (*queue_mem).used_idx;
        let last_seen = KBD_LAST_USED_IDX.load(Ordering::Relaxed);
        (avail_idx, used_idx, last_seen)
    }
}

/// Get the IRQ number for the keyboard device (if initialized)
pub fn get_irq() -> Option<u32> {
    if KBD_INITIALIZED.load(Ordering::Acquire) {
        let slot = unsafe { *(&raw const KBD_DEVICE_SLOT) };
        Some(VIRTIO_IRQ_BASE + slot as u32)
    } else {
        None
    }
}

/// Keyboard interrupt handler.
///
/// Called from the GIC interrupt dispatcher when the keyboard device generates
/// an interrupt. Reports each pending key event to the input core.
pub fn handle_interrupt() {
    if !KBD_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    // Acknowledge the interrupt
    unsafe {
        let base = *(&raw const KBD_DEVICE_BASE);
        if base != 0 {
            let status_addr = (base + 0x60) as *const u32;
            let status = crate::arch_impl::aarch64::mmio::read32(status_addr);
            let ack_addr = (base + 0x64) as *mut u32;
            crate::arch_impl::aarch64::mmio::write32(ack_addr, status);
        }
    }

    // Each key event goes to the input core, which keeps the modifier state
    // and turns keys into console input.
    poll_events();
}

// =============================================================================
// Tablet Polling and Interrupt Handling
// =============================================================================

/// Check if the tablet device is initialized
pub fn is_tablet_initialized() -> bool {
    TABLET_INITIALIZED.load(Ordering::Acquire)
}

/// Get the IRQ number for the tablet device (if initialized)
pub fn get_tablet_irq() -> Option<u32> {
    if TABLET_INITIALIZED.load(Ordering::Acquire) {
        let slot = unsafe { *(&raw const TABLET_DEVICE_SLOT) };
        Some(VIRTIO_IRQ_BASE + slot as u32)
    } else {
        None
    }
}

/// Get current mouse position in screen coordinates.
pub fn mouse_position() -> (u32, u32) {
    (
        MOUSE_X.load(Ordering::Relaxed),
        MOUSE_Y.load(Ordering::Relaxed),
    )
}

/// Get current mouse position and button state.
pub fn mouse_state() -> (u32, u32, u32) {
    (
        MOUSE_X.load(Ordering::Relaxed),
        MOUSE_Y.load(Ordering::Relaxed),
        MOUSE_BUTTONS.load(Ordering::Relaxed),
    )
}

/// Put the pointer at `(x, y)`, clamped to the screen, as if the tablet had moved there
/// (test point 17, `move-pointer`).
pub fn move_to(x: u32, y: u32) {
    let (sw, sh) = screen_dimensions();
    MOUSE_X.store(x.min(sw.saturating_sub(1)), Ordering::Relaxed);
    MOUSE_Y.store(y.min(sh.saturating_sub(1)), Ordering::Relaxed);
}

/// Tablet interrupt handler.
///
/// Processes EV_ABS (mouse movement) and EV_KEY (mouse buttons).
/// Mouse position is stored in atomics for the render thread to read.
/// Mouse clicks are dispatched to the terminal manager for tab switching.
pub fn handle_tablet_interrupt() {
    if !TABLET_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    // Acknowledge the interrupt
    unsafe {
        let base = *(&raw const TABLET_DEVICE_BASE);
        if base != 0 {
            let status_addr = (base + 0x60) as *const u32;
            let status = crate::arch_impl::aarch64::mmio::read32(status_addr);
            let ack_addr = (base + 0x64) as *mut u32;
            crate::arch_impl::aarch64::mmio::write32(ack_addr, status);
        }
    }

    poll_completions(true);
}

fn report_tablet_event(event: VirtioInputEvent) {
        match event.event_type {
            event_type::EV_ABS => match event.code {
                abs_code::ABS_X => {
                    let (sw, _) = screen_dimensions();
                    let x = (event.value as u64 * sw as u64 / (TABLET_ABS_MAX as u64 + 1)) as u32;
                    MOUSE_X.store(x.min(sw - 1), Ordering::Relaxed);
                }
                abs_code::ABS_Y => {
                    let (_, sh) = screen_dimensions();
                    let y = (event.value as u64 * sh as u64 / (TABLET_ABS_MAX as u64 + 1)) as u32;
                    MOUSE_Y.store(y.min(sh - 1), Ordering::Relaxed);
                }
                _ => {}
            },
            event_type::EV_KEY => {
                if event.code == btn_code::BTN_LEFT {
                    let pressed = event.value != 0;
                    if pressed {
                        MOUSE_BUTTONS.store(1, Ordering::Relaxed);
                    } else {
                        MOUSE_BUTTONS.store(0, Ordering::Relaxed);
                    }
                }
            }
            _ => {} // EV_SYN and others — ignore
        }
}

/// A partial virtio frame survives IRQ boundaries. The shared handoff copies it
/// at SYN_REPORT; the task service allocates stream records and routes console keys.
fn report_event(tablet: bool, event: VirtioInputEvent) {
    let mut bytes = [0; 8];
    bytes[..2].copy_from_slice(&event.event_type.to_le_bytes());
    bytes[2..4].copy_from_slice(&event.code.to_le_bytes());
    bytes[4..].copy_from_slice(&event.value.to_le_bytes());
    if let Some(event) = input_decode::decode(bytes) {
        let frame = if tablet { &TABLET_FRAME } else { &KBD_FRAME };
        let device = if tablet {
            TABLET_INPUT.get()
        } else {
            KBD_INPUT.get()
        };
        if let Some(device) = device {
            crate::arch_without_interrupts(|| {
                if let Some((events, overflow)) = frame.lock().push(event) {
                    crate::input::stream::publish_hardware(*device, events, overflow);
                }
            });
        }
    }
}

static KBD_QUEUE_SIZE: AtomicU16 = AtomicU16::new(0);
static TABLET_QUEUE_SIZE: AtomicU16 = AtomicU16::new(0);
static COMPLETION_GATE: spin::Mutex<()> = spin::Mutex::new(());

/// Reset returns the bound ring's DMA buffers before the replay writes them.
/// Recorded completions then read those buffers and repost their descriptors
/// through complete_buffer, before DRIVER_OK returns custody to the device.
fn replay(tablet: bool, events: &[VirtioInputEvent], suppress: bool) -> Result<crate::input::control::Receipt, u64> {
    use crate::syscall::errno::{EIO, ENODEV};
    if if tablet {
        !is_tablet_initialized()
    } else {
        !is_initialized()
    } {
        return Err(ENODEV as u64);
    }
    crate::arch_without_interrupts(|| {
        let _gate = COMPLETION_GATE.lock();
        let input = if tablet { TABLET_INPUT.get() } else { KBD_INPUT.get() }.ok_or(ENODEV as u64)?;
        let scope = crate::test_control::InputReplay::begin(input.id, suppress)?;
        let slot = unsafe {
            if tablet {
                TABLET_DEVICE_SLOT
            } else {
                KBD_DEVICE_SLOT
            }
        };
        let mut device = VirtioMmioDevice::probe(VIRTIO_MMIO_BASE + slot as u64 * VIRTIO_MMIO_SIZE)
            .ok_or(ENODEV as u64)?;
        device.reset();
        let mut reset = false;
        for _ in 0..1000 {
            if device.read_status() == 0 {
                reset = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !reset {
            return Err(EIO as u64);
        }
        unsafe {
            let posted = if tablet {
                TABLET_LAST_USED_IDX.store(0, Ordering::Release);
                core::ptr::write_volatile(&raw mut TABLET_QUEUE_MEM.used_idx, 0);
                *TABLET_FRAME.lock() = Frame::new();
                init_input_device_queue(
                    &mut device,
                    (&raw mut TABLET_QUEUE_MEM).cast(),
                    (&raw const TABLET_EVENT_BUFFERS).cast::<VirtioInputEvent>() as u64,
                    (&raw mut TABLET_QUEUE_MEM).cast(),
                    core::mem::offset_of!(TabletQueueMemory, avail_flags) as u64,
                    core::mem::offset_of!(TabletQueueMemory, used_flags) as u64,
                    NUM_TABLET_BUFFERS,
                    &raw mut TABLET_DEVICE_BASE,
                    Some(events),
                )
            } else {
                KBD_LAST_USED_IDX.store(0, Ordering::Release);
                core::ptr::write_volatile(&raw mut KBD_QUEUE_MEM.used_idx, 0);
                *KBD_FRAME.lock() = Frame::new();
                init_input_device_queue(
                    &mut device,
                    (&raw mut KBD_QUEUE_MEM).cast(),
                    (&raw const KBD_EVENT_BUFFERS).cast::<VirtioInputEvent>() as u64,
                    (&raw mut KBD_QUEUE_MEM).cast(),
                    core::mem::offset_of!(KbdQueueMemory, avail_flags) as u64,
                    core::mem::offset_of!(KbdQueueMemory, used_flags) as u64,
                    NUM_KBD_BUFFERS,
                    &raw mut KBD_DEVICE_BASE,
                    Some(events),
                )
            }
            .map_err(|_| EIO as u64)?;
            device.select_queue(0);
            let size = (if tablet {
                NUM_TABLET_BUFFERS
            } else {
                NUM_KBD_BUFFERS
            })
            .min(device.get_queue_num_max() as usize) as u32;
            Ok(crate::input::control::Receipt { calls: scope.calls(), posted, ring: size })
        }
    })
}

struct ReplayDriver;
static REPLAY: ReplayDriver = ReplayDriver;
impl crate::input::control::Replay for ReplayDriver {
    fn replay_keyboard(&self, suppress: bool) -> Result<crate::input::control::Receipt, u64> {
        replay(false, &[recorded(1, 30, 1), recorded(1, 30, 0), recorded(0, 0, 0)], suppress)
    }
    fn replay_pointer(&self, x: i32, y: i32, suppress: bool) -> Result<crate::input::control::Receipt, u64> {
        let (width, height) = screen_dimensions();
        let ax = crate::input::host::absolute_value(x as u32, width).map_err(|_| crate::syscall::errno::EINVAL as u64)?;
        let ay = crate::input::host::absolute_value(y as u32, height).map_err(|_| crate::syscall::errno::EINVAL as u64)?;
        replay(true, &[recorded(3, 0, ax), recorded(3, 1, ay), recorded(1, 272, 1),
            recorded(1, 272, 0), recorded(0, 0, 0)], suppress)
    }
}
fn recorded(event_type: u16, code: u16, value: i32) -> VirtioInputEvent {
    VirtioInputEvent { event_type, code, value: value as u32 }
}
