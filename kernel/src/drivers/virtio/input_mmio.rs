//! VirtIO Input (Keyboard) driver for ARM64 MMIO transport.
//!
//! This driver handles VirtIO input devices (device ID 18) which provide
//! keyboard, mouse, and other input events. For now, we focus on keyboard.
//!
//! The virtio-input device is simpler than GPU/block - it just sends events
//! to the guest via a single queue. Events use the Linux evdev format:
//! - type: event type (EV_KEY=1 for keyboard)
//! - code: scancode or key code
//! - value: 1=press, 0=release

#![cfg(target_arch = "aarch64")]

use core::sync::atomic::{AtomicBool, AtomicU16, Ordering, fence};
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

// =============================================================================
// Static Memory for VirtIO Queue and Buffers
// =============================================================================

/// Number of event buffers to keep posted
const NUM_EVENT_BUFFERS: usize = 16;

/// Size of the event structure
const EVENT_SIZE: usize = core::mem::size_of::<VirtioInputEvent>();

/// Event queue memory (descriptors, available ring, used ring)
/// For VirtIO legacy (v1), the used ring must be page-aligned (4096 bytes).
/// Layout:
/// - Descriptors: NUM_EVENT_BUFFERS * 16 = 256 bytes
/// - Available ring: 6 + NUM_EVENT_BUFFERS * 2 = 38 bytes
/// - Padding: to 4096 byte boundary
/// - Used ring: 6 + NUM_EVENT_BUFFERS * 8 = 134 bytes
#[repr(C, align(4096))]
struct QueueMemory {
    /// Descriptor table (16 bytes each)
    descriptors: [[u8; 16]; NUM_EVENT_BUFFERS],  // 256 bytes
    /// Available ring
    avail_flags: u16,
    avail_idx: u16,
    avail_ring: [u16; NUM_EVENT_BUFFERS],  // 32 bytes
    avail_used_event: u16,
    /// Padding to align used ring to 4096 for legacy VirtIO
    /// Total so far: 256 + 4 + 32 + 2 = 294 bytes, need to pad to 4096
    _pad_to_page: [u8; 4096 - 294],
    /// Used ring (must be page-aligned for legacy devices)
    used_flags: u16,
    used_idx: u16,
    used_ring: [[u8; 8]; NUM_EVENT_BUFFERS], // id (u32) + len (u32)
    used_avail_event: u16,
}

/// Event buffers to receive input events
#[repr(C, align(4096))]
struct EventBuffers {
    events: [VirtioInputEvent; NUM_EVENT_BUFFERS],
}

// Static memory
static mut QUEUE_MEM: QueueMemory = QueueMemory {
    descriptors: [[0; 16]; NUM_EVENT_BUFFERS],
    avail_flags: 0,
    avail_idx: 0,
    avail_ring: [0; NUM_EVENT_BUFFERS],
    avail_used_event: 0,
    _pad_to_page: [0; 4096 - 294],
    used_flags: 0,
    used_idx: 0,
    used_ring: [[0; 8]; NUM_EVENT_BUFFERS],
    used_avail_event: 0,
};

static mut EVENT_BUFFERS: EventBuffers = EventBuffers {
    events: [VirtioInputEvent::empty(); NUM_EVENT_BUFFERS],
};

// Device state
static mut DEVICE_BASE: u64 = 0;
static DEVICE_INITIALIZED: AtomicBool = AtomicBool::new(false);
static LAST_USED_IDX: AtomicU16 = AtomicU16::new(0);

// =============================================================================
// Descriptor Helpers
// =============================================================================

fn write_descriptor(idx: usize, addr: u64, len: u32, flags: u16, next: u16) {
    unsafe {
        let queue_mem = &raw mut QUEUE_MEM;
        let desc = &mut (*queue_mem).descriptors[idx];
        // addr (8 bytes)
        desc[0..8].copy_from_slice(&addr.to_le_bytes());
        // len (4 bytes)
        desc[8..12].copy_from_slice(&len.to_le_bytes());
        // flags (2 bytes)
        desc[12..14].copy_from_slice(&flags.to_le_bytes());
        // next (2 bytes)
        desc[14..16].copy_from_slice(&next.to_le_bytes());
    }
}

// =============================================================================
// Driver Implementation
// =============================================================================

/// Initialize the VirtIO input driver
pub fn init() -> Result<(), &'static str> {
    if DEVICE_INITIALIZED.load(Ordering::Relaxed) {
        return Ok(());
    }

    crate::serial_println!("[virtio-input] Searching for input device (ID={})...", device_id::INPUT);

    // Search for VirtIO input device - show all devices found
    for i in 0..VIRTIO_MMIO_COUNT {
        let base = VIRTIO_MMIO_BASE + (i as u64) * VIRTIO_MMIO_SIZE;

        if let Some(mut device) = VirtioMmioDevice::probe(base) {
            let id = device.device_id();
            crate::serial_println!("[virtio-input] Slot {} at {:#x}: device_id={}", i, base, id);

            if id == device_id::INPUT {
                crate::serial_println!("[virtio-input] Found input device at {:#x}", base);
                crate::serial_println!("[virtio-input] Device version: {}", device.version());

                // Initialize the device
                init_device(&mut device)?;

                unsafe { *(&raw mut DEVICE_BASE) = base; }
                DEVICE_INITIALIZED.store(true, Ordering::Release);

                crate::serial_println!("[virtio-input] Input device initialized successfully");
                return Ok(());
            }
        }
    }

    Err("No VirtIO input device found")
}

fn init_device(device: &mut VirtioMmioDevice) -> Result<(), &'static str> {
    let version = device.version();

    // For v1 (legacy), set guest page size before queue setup
    if version == 1 {
        device.set_guest_page_size(4096);
    }

    // Initialize device (reset, ACKNOWLEDGE, DRIVER, features, FEATURES_OK)
    device.init(0)?; // No special features needed for input

    // Select queue 0 (event queue) and get max size
    device.select_queue(0);
    let queue_size = device.get_queue_num_max();
    crate::serial_println!("[virtio-input] Event queue max size: {}", queue_size);

    if queue_size == 0 {
        return Err("Event queue not available");
    }

    // Use our static buffer size
    let actual_size = NUM_EVENT_BUFFERS.min(queue_size as usize);

    // Set queue size
    device.set_queue_num(actual_size as u32);

    // Setup the event queue
    unsafe {
        let queue_addr = &raw const QUEUE_MEM as *const _ as u64;
        let desc_addr = queue_addr;
        let avail_addr = queue_addr + core::mem::offset_of!(QueueMemory, avail_flags) as u64;
        let used_addr = queue_addr + core::mem::offset_of!(QueueMemory, used_flags) as u64;

        if version == 1 {
            // Legacy: use PFN-based setup
            let pfn = (queue_addr >> 12) as u32;
            device.set_queue_align(4096);
            device.set_queue_pfn(pfn);
        } else {
            // Modern: use separate addresses
            device.set_queue_desc(desc_addr);
            device.set_queue_avail(avail_addr);
            device.set_queue_used(used_addr);
            device.set_queue_ready(true);
        }
    }

    // Post event buffers to the queue
    unsafe {
        post_event_buffers(actual_size);
    }

    // Mark driver ready
    device.driver_ok();

    Ok(())
}

/// Post event buffers to the available ring
unsafe fn post_event_buffers(count: usize) {
    let event_base = (&raw const EVENT_BUFFERS).cast::<VirtioInputEvent>() as u64;
    let queue_mem = &raw mut QUEUE_MEM;
    let device_base = &raw const DEVICE_BASE;

    for i in 0..count {
        // Setup descriptor: device writes events to our buffer
        let event_addr = event_base + (i * EVENT_SIZE) as u64;
        write_descriptor(i, event_addr, EVENT_SIZE as u32, DESC_F_WRITE, 0);

        // Add to available ring
        (*queue_mem).avail_ring[i] = i as u16;
    }

    // Update available index
    fence(Ordering::SeqCst);
    (*queue_mem).avail_idx = count as u16;
    fence(Ordering::SeqCst);

    // Notify device (queue 0)
    let base = *device_base;
    if base != 0 {
        let notify_addr = (base + 0x50) as *mut u32;
        core::ptr::write_volatile(notify_addr, 0);
    }
}

/// Poll for new input events
///
/// Returns an iterator over any pending events.
pub fn poll_events() -> impl Iterator<Item = VirtioInputEvent> {
    let mut events = [VirtioInputEvent::empty(); NUM_EVENT_BUFFERS];
    let mut count = 0;

    if !DEVICE_INITIALIZED.load(Ordering::Acquire) {
        return EventIterator { events, count, index: 0 };
    }

    unsafe {
        // Get raw pointers to statics to avoid shared references to mutable statics
        let queue_mem = &raw mut QUEUE_MEM;
        let event_buffers = &raw const EVENT_BUFFERS;
        let device_base = &raw const DEVICE_BASE;

        let last_seen = LAST_USED_IDX.load(Ordering::Relaxed);

        fence(Ordering::SeqCst);
        let current_used = (*queue_mem).used_idx;
        fence(Ordering::SeqCst);

        if current_used != last_seen {
            // Process new events
            let mut idx = last_seen;
            while idx != current_used && count < NUM_EVENT_BUFFERS {
                let ring_idx = (idx as usize) % NUM_EVENT_BUFFERS;

                // Read used ring entry to get descriptor index
                let used_entry = &(*queue_mem).used_ring[ring_idx];
                let desc_idx = u32::from_le_bytes([used_entry[0], used_entry[1], used_entry[2], used_entry[3]]) as usize;

                if desc_idx < NUM_EVENT_BUFFERS {
                    // Copy event
                    events[count] = (*event_buffers).events[desc_idx];
                    count += 1;

                    // Re-post the buffer
                    let avail_idx = (*queue_mem).avail_idx as usize;
                    (*queue_mem).avail_ring[avail_idx % NUM_EVENT_BUFFERS] = desc_idx as u16;
                    fence(Ordering::SeqCst);
                    (*queue_mem).avail_idx = (*queue_mem).avail_idx.wrapping_add(1);
                }

                idx = idx.wrapping_add(1);
            }

            LAST_USED_IDX.store(current_used, Ordering::Release);

            // Notify device of re-posted buffers
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
    events: [VirtioInputEvent; NUM_EVENT_BUFFERS],
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

/// Check if the input device is initialized
pub fn is_initialized() -> bool {
    DEVICE_INITIALIZED.load(Ordering::Acquire)
}

/// Debug: get current ring state (avail_idx, used_idx, last_seen)
pub fn debug_ring_state() -> (u16, u16, u16) {
    if !DEVICE_INITIALIZED.load(Ordering::Acquire) {
        return (0, 0, 0);
    }
    unsafe {
        let queue_mem = &raw const QUEUE_MEM;
        let avail_idx = (*queue_mem).avail_idx;
        let used_idx = (*queue_mem).used_idx;
        let last_seen = LAST_USED_IDX.load(Ordering::Relaxed);
        (avail_idx, used_idx, last_seen)
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

/// Check if keycode is left or right shift
pub fn is_shift(code: u16) -> bool {
    code == 42 || code == 54
}
