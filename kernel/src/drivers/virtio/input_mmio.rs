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

use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering, fence};
use super::mmio::{VirtioMmioDevice, device_id, VIRTIO_MMIO_BASE, VIRTIO_MMIO_SIZE, VIRTIO_MMIO_COUNT};

/// VirtIO descriptor flags
const DESC_F_WRITE: u16 = 2;  // Device writes (vs reads)
#[allow(dead_code)]
const DESC_F_NEXT: u16 = 1;   // Descriptor continues via next field

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
    pub const EV_SYN: u16 = 0x00;  // Synchronization
    pub const EV_KEY: u16 = 0x01;  // Key press/release
    pub const EV_REL: u16 = 0x02;  // Relative movement (mouse)
    pub const EV_ABS: u16 = 0x03;  // Absolute position
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
    descriptors: [[u8; 16]; NUM_KBD_BUFFERS],   // 1024 bytes
    /// Available ring
    avail_flags: u16,
    avail_idx: u16,
    avail_ring: [u16; NUM_KBD_BUFFERS],          // 128 bytes
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
    descriptors: [[u8; 16]; NUM_TABLET_BUFFERS],  // 256 bytes
    avail_flags: u16,
    avail_idx: u16,
    avail_ring: [u16; NUM_TABLET_BUFFERS],         // 32 bytes
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

/// Screen dimensions for coordinate scaling (VirtIO GPU is 1280x800)
const SCREEN_WIDTH: u32 = 1280;
const SCREEN_HEIGHT: u32 = 800;

/// Tablet absolute position range (0..32767 for virtio-tablet-device)
const TABLET_ABS_MAX: u32 = 32767;

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
unsafe fn write_descriptor_at(desc_table: *mut [u8; 16], idx: usize, addr: u64, len: u32, flags: u16, next: u16) {
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
        core::ptr::write_volatile(config_select, input_cfg::EV_BITS);
        core::ptr::write_volatile(config_subsel, event_type::EV_ABS as u8);

        // If size > 0, the device reports supported ABS axis codes → it's a pointer
        let size = core::ptr::read_volatile(config_size);
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

    crate::serial_println!("[virtio-input] Searching for input devices (ID={})...", device_id::INPUT);

    for i in 0..VIRTIO_MMIO_COUNT {
        let base = VIRTIO_MMIO_BASE + (i as u64) * VIRTIO_MMIO_SIZE;

        if let Some(mut device) = VirtioMmioDevice::probe(base) {
            let id = device.device_id();
            crate::serial_println!("[virtio-input] Slot {} at {:#x}: device_id={}", i, base, id);

            if id == device_id::INPUT {
                if is_pointing_device(&device) {
                    if !TABLET_INITIALIZED.load(Ordering::Relaxed) {
                        crate::serial_println!("[virtio-input] Found tablet at {:#x} (slot {}, supports EV_ABS)", base, i);
                        init_tablet_device(&mut device, i)?;
                    }
                } else if !KBD_INITIALIZED.load(Ordering::Relaxed) {
                    crate::serial_println!("[virtio-input] Found keyboard at {:#x} (slot {}, no EV_ABS)", base, i);
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
    crate::serial_println!("[virtio-input] Keyboard device version: {}", device.version());

    init_input_device_queue(
        device,
        (&raw mut KBD_QUEUE_MEM) as *mut u8,
        (&raw const KBD_EVENT_BUFFERS).cast::<VirtioInputEvent>() as u64,
        (&raw mut KBD_QUEUE_MEM).cast::<[u8; 16]>(),
        core::mem::offset_of!(KbdQueueMemory, avail_flags) as u64,
        core::mem::offset_of!(KbdQueueMemory, used_flags) as u64,
        NUM_KBD_BUFFERS,
        &raw mut KBD_DEVICE_BASE,
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

    KBD_INITIALIZED.store(true, Ordering::Release);
    crate::serial_println!("[virtio-input] Keyboard initialized ({} buffers)", NUM_KBD_BUFFERS);
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

    TABLET_INITIALIZED.store(true, Ordering::Release);
    crate::serial_println!("[virtio-input] Tablet initialized ({} buffers)", NUM_TABLET_BUFFERS);
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
) -> Result<(), &'static str> {
    let version = device.version();

    if version == 1 {
        device.set_guest_page_size(4096);
    }

    device.init(0)?;

    device.select_queue(0);
    let queue_size = device.get_queue_num_max();
    crate::serial_println!("[virtio-input] Event queue max size: {}", queue_size);

    if queue_size == 0 {
        return Err("Event queue not available");
    }

    let actual_size = num_buffers.min(queue_size as usize);
    device.set_queue_num(actual_size as u32);

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
            write_descriptor_at(desc_table, i, event_addr, EVENT_SIZE as u32, DESC_F_WRITE, 0);
        }
    }

    // Write available ring (the avail_ring starts right after avail_flags(2) + avail_idx(2))
    // We need to write directly to the queue memory
    // The avail_ring array is at offset avail_offset + 4
    unsafe {
        let avail_idx_ptr = (queue_mem_base as u64 + avail_offset + 2) as *mut u16;
        let avail_ring_ptr = (queue_mem_base as u64 + avail_offset + 4) as *mut u16;

        for i in 0..actual_size {
            core::ptr::write_volatile(avail_ring_ptr.add(i), i as u16);
        }

        fence(Ordering::SeqCst);
        core::ptr::write_volatile(avail_idx_ptr, actual_size as u16);
        fence(Ordering::SeqCst);
    }

    // Notify device (queue 0)
    // device.base() is already the virtual MMIO address (probe adds phys_offset)
    let virt_base = device.base();
    unsafe {
        *device_base_ptr = virt_base;
        let notify_addr = (virt_base + 0x50) as *mut u32;
        core::ptr::write_volatile(notify_addr, 0);
    }

    device.driver_ok();
    Ok(())
}

// =============================================================================
// Keyboard Polling and Interrupt Handling
// =============================================================================

/// Poll for new keyboard input events.
pub fn poll_events() -> impl Iterator<Item = VirtioInputEvent> {
    let mut events = [VirtioInputEvent::empty(); NUM_KBD_BUFFERS];
    let mut count = 0;

    if !KBD_INITIALIZED.load(Ordering::Acquire) {
        return EventIterator { events, count, index: 0 };
    }

    unsafe {
        let queue_mem = &raw mut KBD_QUEUE_MEM;
        let event_buffers = &raw const KBD_EVENT_BUFFERS;
        let device_base = &raw const KBD_DEVICE_BASE;

        let last_seen = KBD_LAST_USED_IDX.load(Ordering::Relaxed);

        fence(Ordering::SeqCst);
        let current_used = (*queue_mem).used_idx;
        fence(Ordering::SeqCst);

        if current_used != last_seen {
            let mut idx = last_seen;
            while idx != current_used && count < NUM_KBD_BUFFERS {
                let ring_idx = (idx as usize) % NUM_KBD_BUFFERS;
                let used_entry = &(*queue_mem).used_ring[ring_idx];
                let desc_idx = u32::from_le_bytes([used_entry[0], used_entry[1], used_entry[2], used_entry[3]]) as usize;

                if desc_idx < NUM_KBD_BUFFERS {
                    events[count] = (*event_buffers).events[desc_idx];
                    count += 1;

                    let avail_idx = (*queue_mem).avail_idx as usize;
                    (*queue_mem).avail_ring[avail_idx % NUM_KBD_BUFFERS] = desc_idx as u16;
                    fence(Ordering::SeqCst);
                    (*queue_mem).avail_idx = (*queue_mem).avail_idx.wrapping_add(1);
                }

                idx = idx.wrapping_add(1);
            }

            KBD_LAST_USED_IDX.store(current_used, Ordering::Release);

            let base = *device_base;
            if count > 0 && base != 0 {
                fence(Ordering::SeqCst);
                let notify_addr = (base + 0x50) as *mut u32;
                core::ptr::write_volatile(notify_addr, 0);
            }
        }
    }

    EventIterator { events, count, index: 0 }
}

struct EventIterator {
    events: [VirtioInputEvent; NUM_KBD_BUFFERS],
    count: usize,
    index: usize,
}

impl Iterator for EventIterator {
    type Item = VirtioInputEvent;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.count {
            let event = self.events[self.index];
            self.index += 1;
            Some(event)
        } else {
            None
        }
    }
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
/// an interrupt. Processes pending events and pushes keyboard characters to stdin.
pub fn handle_interrupt() {
    if !KBD_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    // Acknowledge the interrupt
    unsafe {
        let base = *(&raw const KBD_DEVICE_BASE);
        if base != 0 {
            let status_addr = (base + 0x60) as *const u32;
            let status = core::ptr::read_volatile(status_addr);
            let ack_addr = (base + 0x64) as *mut u32;
            core::ptr::write_volatile(ack_addr, status);
        }
    }

    // Track modifier key state
    static SHIFT_PRESSED: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);
    static CTRL_PRESSED: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);
    static CAPS_LOCK_ACTIVE: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);

    // Process all pending events
    for event in poll_events() {
        if event.event_type == event_type::EV_KEY {
            let keycode = event.code;
            let value = event.value; // 0=release, 1=press, 2=repeat

            // Track shift key state
            if is_shift(keycode) {
                SHIFT_PRESSED.store(value != 0, core::sync::atomic::Ordering::Relaxed);
                continue;
            }

            // Track ctrl key state
            if is_ctrl(keycode) {
                CTRL_PRESSED.store(value != 0, core::sync::atomic::Ordering::Relaxed);
                continue;
            }

            // Toggle caps lock on key press only (not repeat or release)
            if keycode == 58 {
                if value == 1 {
                    let prev = CAPS_LOCK_ACTIVE.load(core::sync::atomic::Ordering::Relaxed);
                    CAPS_LOCK_ACTIVE.store(!prev, core::sync::atomic::Ordering::Relaxed);
                }
                continue;
            }

            // Only process key presses and repeats (not releases)
            if value != 0 {
                // Handle function keys for terminal switching (F1=59, F2=60, etc.)
                // Linux evdev keycodes 59-68 match PS/2 scancodes 0x3B-0x44
                if keycode >= 59 && keycode <= 68 {
                    if crate::graphics::terminal_manager::handle_terminal_key(keycode as u8) {
                        continue;
                    }
                }

                // Arrow keys for log scrolling (UP=103, DOWN=108)
                if keycode == 103 || keycode == 108 {
                    if crate::graphics::terminal_manager::handle_logs_arrow_key(keycode as u8) {
                        continue;
                    }
                }

                // Generate VT100 escape sequences for special keys
                // (arrows, Home, End, Delete) that can't be represented
                // as a single character.
                if let Some(seq) = keycode_to_escape_seq(keycode) {
                    for &b in seq {
                        if !crate::tty::push_char_nonblock(b) {
                            crate::ipc::stdin::push_byte_from_irq(b);
                        }
                    }
                    continue;
                }

                let shift = SHIFT_PRESSED.load(core::sync::atomic::Ordering::Relaxed);
                let caps = CAPS_LOCK_ACTIVE.load(core::sync::atomic::Ordering::Relaxed);
                let ctrl = CTRL_PRESSED.load(core::sync::atomic::Ordering::Relaxed);

                // Ctrl+letter -> control character (e.g., Ctrl+C = 0x03)
                let c = if ctrl {
                    ctrl_char_from_keycode(keycode)
                } else {
                    // For letter keys, caps lock XOR shift determines case
                    let effective_shift = if is_letter(keycode) { shift ^ caps } else { shift };
                    keycode_to_char(keycode, effective_shift)
                };

                if let Some(c) = c {
                    // Route through TTY for echo and line discipline processing.
                    if !crate::tty::push_char_nonblock(c as u8) {
                        crate::ipc::stdin::push_byte_from_irq(c as u8);
                    }
                }
            }
        }
    }
}

// =============================================================================
// Tablet Polling and Interrupt Handling
// =============================================================================

/// Poll for new tablet events.
fn poll_tablet_events() -> TabletEventIterator {
    let mut events = [VirtioInputEvent::empty(); NUM_TABLET_BUFFERS];
    let mut count = 0;

    if !TABLET_INITIALIZED.load(Ordering::Acquire) {
        return TabletEventIterator { events, count, index: 0 };
    }

    unsafe {
        let queue_mem = &raw mut TABLET_QUEUE_MEM;
        let event_buffers = &raw const TABLET_EVENT_BUFFERS;
        let device_base = &raw const TABLET_DEVICE_BASE;

        let last_seen = TABLET_LAST_USED_IDX.load(Ordering::Relaxed);

        fence(Ordering::SeqCst);
        let current_used = (*queue_mem).used_idx;
        fence(Ordering::SeqCst);

        if current_used != last_seen {
            let mut idx = last_seen;
            while idx != current_used && count < NUM_TABLET_BUFFERS {
                let ring_idx = (idx as usize) % NUM_TABLET_BUFFERS;
                let used_entry = &(*queue_mem).used_ring[ring_idx];
                let desc_idx = u32::from_le_bytes([used_entry[0], used_entry[1], used_entry[2], used_entry[3]]) as usize;

                if desc_idx < NUM_TABLET_BUFFERS {
                    events[count] = (*event_buffers).events[desc_idx];
                    count += 1;

                    let avail_idx = (*queue_mem).avail_idx as usize;
                    (*queue_mem).avail_ring[avail_idx % NUM_TABLET_BUFFERS] = desc_idx as u16;
                    fence(Ordering::SeqCst);
                    (*queue_mem).avail_idx = (*queue_mem).avail_idx.wrapping_add(1);
                }

                idx = idx.wrapping_add(1);
            }

            TABLET_LAST_USED_IDX.store(current_used, Ordering::Release);

            let base = *device_base;
            if count > 0 && base != 0 {
                fence(Ordering::SeqCst);
                let notify_addr = (base + 0x50) as *mut u32;
                core::ptr::write_volatile(notify_addr, 0);
            }
        }
    }

    TabletEventIterator { events, count, index: 0 }
}

struct TabletEventIterator {
    events: [VirtioInputEvent; NUM_TABLET_BUFFERS],
    count: usize,
    index: usize,
}

impl Iterator for TabletEventIterator {
    type Item = VirtioInputEvent;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.count {
            let event = self.events[self.index];
            self.index += 1;
            Some(event)
        } else {
            None
        }
    }
}

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
    (MOUSE_X.load(Ordering::Relaxed), MOUSE_Y.load(Ordering::Relaxed))
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
            let status = core::ptr::read_volatile(status_addr);
            let ack_addr = (base + 0x64) as *mut u32;
            core::ptr::write_volatile(ack_addr, status);
        }
    }

    for event in poll_tablet_events() {
        match event.event_type {
            event_type::EV_ABS => {
                match event.code {
                    abs_code::ABS_X => {
                        // Scale from 0..32767 to 0..SCREEN_WIDTH-1
                        let x = (event.value as u64 * SCREEN_WIDTH as u64 / (TABLET_ABS_MAX as u64 + 1)) as u32;
                        MOUSE_X.store(x.min(SCREEN_WIDTH - 1), Ordering::Relaxed);
                    }
                    abs_code::ABS_Y => {
                        let y = (event.value as u64 * SCREEN_HEIGHT as u64 / (TABLET_ABS_MAX as u64 + 1)) as u32;
                        MOUSE_Y.store(y.min(SCREEN_HEIGHT - 1), Ordering::Relaxed);
                    }
                    _ => {}
                }
            }
            event_type::EV_KEY => {
                if event.code == btn_code::BTN_LEFT {
                    let pressed = event.value != 0;
                    if pressed {
                        MOUSE_BUTTONS.store(1, Ordering::Relaxed);
                        // Dispatch click to terminal manager for tab switching
                        let x = MOUSE_X.load(Ordering::Relaxed) as usize;
                        let y = MOUSE_Y.load(Ordering::Relaxed) as usize;
                        crate::graphics::terminal_manager::handle_mouse_click(x, y);
                    } else {
                        MOUSE_BUTTONS.store(0, Ordering::Relaxed);
                    }
                }
            }
            _ => {} // EV_SYN and others — ignore
        }
    }
}

// =============================================================================
// Linux Keycode to ASCII Conversion
// =============================================================================

/// Convert Linux keycode to ASCII character
///
/// Linux keycodes are different from PS/2 scancodes. This table handles
/// the most common keys for a basic interactive shell.
pub fn keycode_to_char(code: u16, shift: bool) -> Option<char> {
    // Linux keycode mapping (subset for common keys)
    let c = match code {
        // Row 1: number row
        2 => if shift { '!' } else { '1' },
        3 => if shift { '@' } else { '2' },
        4 => if shift { '#' } else { '3' },
        5 => if shift { '$' } else { '4' },
        6 => if shift { '%' } else { '5' },
        7 => if shift { '^' } else { '6' },
        8 => if shift { '&' } else { '7' },
        9 => if shift { '*' } else { '8' },
        10 => if shift { '(' } else { '9' },
        11 => if shift { ')' } else { '0' },
        12 => if shift { '_' } else { '-' },
        13 => if shift { '+' } else { '=' },
        14 => '\x08', // Backspace

        // Row 2: QWERTY
        15 => '\t',   // Tab
        16 => if shift { 'Q' } else { 'q' },
        17 => if shift { 'W' } else { 'w' },
        18 => if shift { 'E' } else { 'e' },
        19 => if shift { 'R' } else { 'r' },
        20 => if shift { 'T' } else { 't' },
        21 => if shift { 'Y' } else { 'y' },
        22 => if shift { 'U' } else { 'u' },
        23 => if shift { 'I' } else { 'i' },
        24 => if shift { 'O' } else { 'o' },
        25 => if shift { 'P' } else { 'p' },
        26 => if shift { '{' } else { '[' },
        27 => if shift { '}' } else { ']' },
        28 => '\n',   // Enter

        // Row 3: ASDF
        30 => if shift { 'A' } else { 'a' },
        31 => if shift { 'S' } else { 's' },
        32 => if shift { 'D' } else { 'd' },
        33 => if shift { 'F' } else { 'f' },
        34 => if shift { 'G' } else { 'g' },
        35 => if shift { 'H' } else { 'h' },
        36 => if shift { 'J' } else { 'j' },
        37 => if shift { 'K' } else { 'k' },
        38 => if shift { 'L' } else { 'l' },
        39 => if shift { ':' } else { ';' },
        40 => if shift { '"' } else { '\'' },
        41 => if shift { '~' } else { '`' },
        43 => if shift { '|' } else { '\\' },

        // Row 4: ZXCV
        44 => if shift { 'Z' } else { 'z' },
        45 => if shift { 'X' } else { 'x' },
        46 => if shift { 'C' } else { 'c' },
        47 => if shift { 'V' } else { 'v' },
        48 => if shift { 'B' } else { 'b' },
        49 => if shift { 'N' } else { 'n' },
        50 => if shift { 'M' } else { 'm' },
        51 => if shift { '<' } else { ',' },
        52 => if shift { '>' } else { '.' },
        53 => if shift { '?' } else { '/' },

        // Space
        57 => ' ',

        _ => return None,
    };

    Some(c)
}

/// Convert a Linux keycode to a VT100 escape sequence for special keys
/// that require multi-byte output (arrow keys, Home, End, Delete).
fn keycode_to_escape_seq(code: u16) -> Option<&'static [u8]> {
    match code {
        103 => Some(b"\x1b[A"),  // Up
        108 => Some(b"\x1b[B"),  // Down
        106 => Some(b"\x1b[C"),  // Right
        105 => Some(b"\x1b[D"),  // Left
        102 => Some(b"\x1b[H"),  // Home
        107 => Some(b"\x1b[F"),  // End
        111 => Some(b"\x1b[3~"), // Delete
        _ => None,
    }
}

/// Check if a keycode is a modifier key
pub fn is_modifier(code: u16) -> bool {
    matches!(code,
        29 |    // Left Ctrl
        42 |    // Left Shift
        54 |    // Right Shift
        56 |    // Left Alt
        97 |    // Right Ctrl
        100     // Right Alt
    )
}

/// Check if keycode is an alphabetic letter (affected by caps lock)
pub fn is_letter(code: u16) -> bool {
    matches!(code,
        16..=25 |  // Q W E R T Y U I O P
        30..=38 |  // A S D F G H J K L
        44..=50    // Z X C V B N M
    )
}

/// Check if keycode is left or right shift
pub fn is_shift(code: u16) -> bool {
    code == 42 || code == 54
}

/// Check if keycode is left or right ctrl
pub fn is_ctrl(code: u16) -> bool {
    code == 29 || code == 97
}

/// Convert Ctrl+keycode to control character
///
/// When Ctrl is held, alphabetic keys produce control characters:
/// Ctrl+A = 0x01, Ctrl+B = 0x02, Ctrl+C = 0x03, etc.
pub fn ctrl_char_from_keycode(code: u16) -> Option<char> {
    let base_char = match code {
        // QWERTY row
        16 => 'q', 17 => 'w', 18 => 'e', 19 => 'r', 20 => 't',
        21 => 'y', 22 => 'u', 23 => 'i', 24 => 'o', 25 => 'p',
        // ASDF row
        30 => 'a', 31 => 's', 32 => 'd', 33 => 'f', 34 => 'g',
        35 => 'h', 36 => 'j', 37 => 'k', 38 => 'l',
        // ZXCV row
        44 => 'z', 45 => 'x', 46 => 'c', 47 => 'v', 48 => 'b',
        49 => 'n', 50 => 'm',
        // Special
        26 => '[', 27 => ']', 43 => '\\',
        _ => return None,
    };

    let ctrl_code = match base_char {
        'a'..='z' => (base_char as u8) - 0x60,  // 'c' (0x63) -> 0x03
        '[' => 0x1B, '\\' => 0x1C, ']' => 0x1D,
        _ => return None,
    };
    Some(ctrl_code as char)
}
