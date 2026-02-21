//! VirtIO Input Device Driver for ARM64 (PCI Transport)
//!
//! Implements keyboard and mouse input via VirtIO PCI modern transport.
//! Reuses the event processing logic from `input_mmio.rs` but communicates
//! via the PCI transport layer (`VirtioPciDevice` from `pci_transport.rs`).
//!
//! Parallels Desktop exposes a VirtIO PCI device at type 19 (non-standard;
//! standard virtio-input is type 18). This driver accepts both.

#![cfg(target_arch = "aarch64")]

use super::pci_transport::{VirtioPciDevice, device_id, enumerate_virtio_pci_devices};
use super::input_mmio::{
    VirtioInputEvent, event_type, keycode_to_char, keycode_to_escape_seq,
    is_shift, is_ctrl, is_letter, ctrl_char_from_keycode,
};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering, fence};

// =============================================================================
// Constants
// =============================================================================

/// VirtIO standard feature: version 1.0 (mandatory for PCI modern transport)
const VIRTIO_F_VERSION_1: u64 = 1 << 32;

/// Number of event buffers (pre-posted to the device).
/// Each buffer holds one VirtioInputEvent (8 bytes).
/// 64 buffers supports ~21 chars in flight for paste.
const NUM_EVENT_BUFFERS: usize = 64;

/// Size of one VirtioInputEvent in bytes
const EVENT_SIZE: usize = core::mem::size_of::<VirtioInputEvent>();

/// VirtIO input config select values (per VirtIO spec 5.8.4).
#[allow(dead_code)]
mod input_cfg {
    /// Query device name string
    pub const ID_NAME: u8 = 0x01;
    /// Query supported event type bitmaps
    pub const EV_BITS: u8 = 0x11;
}

/// Absolute axis codes for mouse/tablet
mod abs_code {
    pub const ABS_X: u16 = 0x00;
    pub const ABS_Y: u16 = 0x01;
}

/// Button codes for mouse
mod btn_code {
    pub const BTN_LEFT: u16 = 0x110;
}

// =============================================================================
// Virtqueue Structures (same layout as gpu_pci.rs)
// =============================================================================

/// Virtqueue descriptor
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

const DESC_F_WRITE: u16 = 2;

/// Available ring
#[repr(C)]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; NUM_EVENT_BUFFERS],
}

/// Used ring element
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

/// Used ring
#[repr(C)]
struct VirtqUsed {
    flags: u16,
    idx: u16,
    ring: [VirtqUsedElem; NUM_EVENT_BUFFERS],
}

// Event queue memory (page-aligned, used ring at +4096 for alignment)
#[repr(C, align(4096))]
struct InputEventQueueMemory {
    desc: [VirtqDesc; NUM_EVENT_BUFFERS],     // 64 * 16 = 1024 bytes
    avail: VirtqAvail,                         // 4 + 64*2 = 132 bytes
    _padding: [u8; 4096 - 1024 - 132],
    used: VirtqUsed,                           // 4 + 64*8 = 516 bytes
}

/// Event buffers where the device writes input events
#[repr(C, align(64))]
struct InputEventBuffers {
    events: [VirtioInputEvent; NUM_EVENT_BUFFERS],
}

// =============================================================================
// Static Buffers
// =============================================================================

static mut INPUT_EVENT_QUEUE: InputEventQueueMemory = InputEventQueueMemory {
    desc: [VirtqDesc { addr: 0, len: 0, flags: 0, next: 0 }; NUM_EVENT_BUFFERS],
    avail: VirtqAvail { flags: 0, idx: 0, ring: [0; NUM_EVENT_BUFFERS] },
    _padding: [0; 4096 - 1024 - 132],
    used: VirtqUsed {
        flags: 0,
        idx: 0,
        ring: [VirtqUsedElem { id: 0, len: 0 }; NUM_EVENT_BUFFERS],
    },
};

static mut INPUT_EVENT_BUFFERS: InputEventBuffers = InputEventBuffers {
    events: [VirtioInputEvent { event_type: 0, code: 0, value: 0 }; NUM_EVENT_BUFFERS],
};

// =============================================================================
// Device State
// =============================================================================

struct InputPciDeviceState {
    device: VirtioPciDevice,
    last_used_idx: u16,
}

static mut INPUT_PCI_STATE: Option<InputPciDeviceState> = None;
static INPUT_PCI_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Init status for diagnostics (0=not attempted, 1=success, 2=no device, 3=no events, 4=init error, 5=N virtio devices found)
pub static INIT_STATUS: AtomicU32 = AtomicU32::new(0);

/// Mouse position (written by event handler, read by render thread)
static MOUSE_X: AtomicU32 = AtomicU32::new(0);
static MOUSE_Y: AtomicU32 = AtomicU32::new(0);
static MOUSE_BUTTONS: AtomicU32 = AtomicU32::new(0);

/// Tablet absolute position range (0..32767)
const TABLET_ABS_MAX: u32 = 32767;

// =============================================================================
// Helpers
// =============================================================================

#[inline(always)]
fn virt_to_phys(addr: u64) -> u64 {
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    if addr >= HHDM_BASE {
        addr - HHDM_BASE
    } else {
        addr
    }
}

/// Get screen dimensions (fall back to 1280x800)
fn screen_dimensions() -> (u32, u32) {
    if super::gpu_pci::is_initialized() {
        super::gpu_pci::dimensions().unwrap_or((1280, 800))
    } else {
        (1280, 800)
    }
}

// =============================================================================
// Initialization
// =============================================================================

/// Check if the VirtIO Input PCI driver is initialized.
pub fn is_initialized() -> bool {
    INPUT_PCI_INITIALIZED.load(Ordering::Acquire)
}

/// Initialize the VirtIO Input PCI device.
///
/// Searches for a VirtIO input device on the PCI bus. Accepts both
/// standard type 18 (virtio-input) and type 19 (Parallels non-standard).
pub fn init() -> Result<(), &'static str> {
    if is_initialized() {
        return Ok(());
    }

    crate::serial_println!("[virtio-input-pci] Searching for input PCI device...");

    let devices = enumerate_virtio_pci_devices();
    // Store number of VirtIO PCI devices found (shifted into status for diagnostics)
    let num_virtio = devices.len() as u32;
    INIT_STATUS.store(10 + num_virtio, Ordering::Relaxed);

    // Find an input device: try standard type 18 first, then type 19
    let mut target: Option<VirtioPciDevice> = None;
    for dev in devices {
        let pci = dev.pci_device();
        crate::serial_println!(
            "[virtio-input-pci] VirtIO PCI device: type={} pci_id={:#06x}",
            dev.device_id(),
            pci.device_id,
        );

        if dev.device_id() == device_id::INPUT {
            crate::serial_println!("[virtio-input-pci] Found standard virtio-input (type=18)");
            target = Some(dev);
            break;
        }
        if dev.device_id() == 19 {
            // VirtIO type 19 is IOMMU per the VirtIO 1.1+ spec, NOT input.
            // Parallels exposes this device (PCI ID 0x1053) but it does not
            // implement the virtio-input config interface. Skip it.
            crate::serial_println!(
                "[virtio-input-pci] Type=19 is VirtIO IOMMU (not input), skipping"
            );
        }
    }

    let mut virtio = match target {
        Some(d) => d,
        None => {
            INIT_STATUS.store(2, Ordering::Relaxed); // 2 = no device found
            return Err("No VirtIO input PCI device found");
        }
    };

    // Query device-specific config to verify this is an input device.
    // VirtIO input config: write select=EV_BITS subsel=EV_KEY → if size>0, it supports keyboard.
    // This must be done BEFORE init() (config is readable at any status).
    let has_device_cfg = virtio.read_config_u8(0) != 0 || virtio.read_config_u8(2) != 0;
    crate::serial_println!(
        "[virtio-input-pci] Device config probe: has_device_cfg={}",
        has_device_cfg,
    );

    // Try to query EV_BITS for EV_KEY (select=0x11, subsel=0x01)
    // VirtIO input config layout: offset 0=select(w), 1=subsel(w), 2=size(r), 8..=135=data
    // For PCI modern transport, device_cfg maps to the device-specific config region.
    // Write select and subsel, then read size.
    virtio.write_config_u8(0, input_cfg::EV_BITS);
    virtio.write_config_u8(1, event_type::EV_KEY as u8);
    let ev_key_size = virtio.read_config_u8(2);
    crate::serial_println!(
        "[virtio-input-pci] EV_KEY bitmap size: {} bytes",
        ev_key_size,
    );

    // Also check EV_ABS for mouse/tablet capability
    virtio.write_config_u8(0, input_cfg::EV_BITS);
    virtio.write_config_u8(1, event_type::EV_ABS as u8);
    let ev_abs_size = virtio.read_config_u8(2);
    crate::serial_println!(
        "[virtio-input-pci] EV_ABS bitmap size: {} bytes",
        ev_abs_size,
    );

    // Query device name
    virtio.write_config_u8(0, input_cfg::ID_NAME);
    virtio.write_config_u8(1, 0);
    let name_size = virtio.read_config_u8(2) as usize;
    if name_size > 0 {
        let mut name_buf = [0u8; 64];
        let to_read = name_size.min(64);
        for i in 0..to_read {
            name_buf[i] = virtio.read_config_u8(8 + i);
        }
        if let Ok(name) = core::str::from_utf8(&name_buf[..to_read]) {
            crate::serial_println!("[virtio-input-pci] Device name: {}", name.trim_end_matches('\0'));
        }
    }

    if ev_key_size == 0 && ev_abs_size == 0 {
        return Err("VirtIO PCI device does not support keyboard or mouse events");
    }

    // Initialize VirtIO device (reset, features, etc.)
    virtio.init(VIRTIO_F_VERSION_1)?;
    crate::serial_println!("[virtio-input-pci] VirtIO init complete (features negotiated)");

    // Set up event queue (queue 0)
    virtio.select_queue(0);
    let queue_max = virtio.get_queue_num_max();
    crate::serial_println!("[virtio-input-pci] Event queue max size: {}", queue_max);

    if queue_max == 0 {
        return Err("Event queue size is 0");
    }

    let queue_size = core::cmp::min(queue_max as usize, NUM_EVENT_BUFFERS);
    virtio.set_queue_num(queue_size as u32);

    let queue_phys = virt_to_phys(&raw const INPUT_EVENT_QUEUE as u64);
    let events_phys = virt_to_phys(&raw const INPUT_EVENT_BUFFERS as u64);

    // Initialize descriptors: each points to one event buffer, device-writable
    unsafe {
        let q = &raw mut INPUT_EVENT_QUEUE;
        for i in 0..queue_size {
            let event_phys = events_phys + (i * EVENT_SIZE) as u64;
            (*q).desc[i] = VirtqDesc {
                addr: event_phys,
                len: EVENT_SIZE as u32,
                flags: DESC_F_WRITE,
                next: 0,
            };
        }

        // Post all buffers to available ring
        for i in 0..queue_size {
            (*q).avail.ring[i] = i as u16;
        }
        fence(Ordering::SeqCst);
        (*q).avail.idx = queue_size as u16;
        fence(Ordering::SeqCst);

        (*q).used.flags = 0;
        (*q).used.idx = 0;
    }

    // Set queue memory addresses (PCI modern: separate desc/avail/used)
    virtio.set_queue_desc(queue_phys);
    // avail ring is right after descriptors: 64 descs * 16 bytes = 1024
    virtio.set_queue_avail(queue_phys + 1024);
    // used ring is at the page boundary (+4096)
    virtio.set_queue_used(queue_phys + 4096);
    virtio.set_queue_ready(true);

    crate::serial_println!(
        "[virtio-input-pci] Queue: desc={:#x} avail={:#x} used={:#x} size={}",
        queue_phys, queue_phys + 1024, queue_phys + 4096, queue_size,
    );

    // Mark device ready
    virtio.driver_ok();

    // Notify device that buffers are available (queue 0)
    virtio.notify_queue(0);

    crate::serial_println!(
        "[virtio-input-pci] Device ready (kbd={} mouse={})",
        ev_key_size > 0,
        ev_abs_size > 0,
    );

    // Store state
    unsafe {
        let ptr = &raw mut INPUT_PCI_STATE;
        *ptr = Some(InputPciDeviceState {
            device: virtio,
            last_used_idx: 0,
        });
    }
    INPUT_PCI_INITIALIZED.store(true, Ordering::Release);

    crate::serial_println!("[virtio-input-pci] Initialized with {} event buffers", queue_size);
    Ok(())
}

// =============================================================================
// Event Polling
// =============================================================================

/// Poll for new input events from the VirtIO input device.
///
/// Called from the timer interrupt handler. Processes all pending events
/// in the used ring, dispatches keyboard characters to TTY, and re-posts
/// consumed buffers to the available ring.
pub fn poll_events() {
    if !INPUT_PCI_INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    unsafe {
        let state_ptr = &raw mut INPUT_PCI_STATE;
        let state = match (*state_ptr).as_mut() {
            Some(s) => s,
            None => return,
        };

        let q = &raw mut INPUT_EVENT_QUEUE;
        let events = &raw const INPUT_EVENT_BUFFERS;

        fence(Ordering::SeqCst);
        let current_used = core::ptr::read_volatile(&(*q).used.idx);
        fence(Ordering::SeqCst);

        if current_used == state.last_used_idx {
            return; // No new events
        }

        let mut idx = state.last_used_idx;
        while idx != current_used {
            let ring_idx = (idx as usize) % NUM_EVENT_BUFFERS;
            let used_elem = core::ptr::read_volatile(&(*q).used.ring[ring_idx]);
            let desc_idx = used_elem.id as usize;

            if desc_idx < NUM_EVENT_BUFFERS {
                let event = core::ptr::read_volatile(&(*events).events[desc_idx]);
                process_event(&event);

                // Re-post this buffer to the available ring
                let avail_idx = core::ptr::read_volatile(&(*q).avail.idx) as usize;
                core::ptr::write_volatile(
                    &mut (*q).avail.ring[avail_idx % NUM_EVENT_BUFFERS],
                    desc_idx as u16,
                );
                fence(Ordering::SeqCst);
                core::ptr::write_volatile(
                    &mut (*q).avail.idx,
                    (avail_idx as u16).wrapping_add(1),
                );
            }

            idx = idx.wrapping_add(1);
        }

        state.last_used_idx = current_used;

        // Notify device that new buffers are available
        if current_used != state.last_used_idx.wrapping_sub(1) {
            fence(Ordering::SeqCst);
            state.device.notify_queue(0);
        }
    }
}

// =============================================================================
// Event Processing
// =============================================================================

/// Modifier key state (shared across poll calls)
static SHIFT_PRESSED: AtomicBool = AtomicBool::new(false);
static CTRL_PRESSED: AtomicBool = AtomicBool::new(false);
static CAPS_LOCK_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Counter for keyboard events received
static KBD_EVENT_COUNT: AtomicU32 = AtomicU32::new(0);

/// Get the count of keyboard events received
pub fn kbd_event_count() -> u32 {
    KBD_EVENT_COUNT.load(Ordering::Relaxed)
}

/// Process a single VirtIO input event.
fn process_event(event: &VirtioInputEvent) {
    match event.event_type {
        event_type::EV_KEY => {
            process_key_event(event.code, event.value);
        }
        event_type::EV_ABS => {
            process_abs_event(event.code, event.value);
        }
        event_type::EV_SYN => {
            // Sync event — ignore
        }
        _ => {
            // Unknown event type — ignore
        }
    }
}

/// Process a keyboard key event.
fn process_key_event(keycode: u16, value: u32) {
    // Mouse button events use EV_KEY with button codes >= 0x110
    if keycode >= 0x110 {
        if keycode == btn_code::BTN_LEFT {
            MOUSE_BUTTONS.store(if value != 0 { 1 } else { 0 }, Ordering::Relaxed);
        }
        return;
    }

    KBD_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);

    // Track shift key state
    if is_shift(keycode) {
        SHIFT_PRESSED.store(value != 0, Ordering::Relaxed);
        return;
    }

    // Track ctrl key state
    if is_ctrl(keycode) {
        CTRL_PRESSED.store(value != 0, Ordering::Relaxed);
        return;
    }

    // Toggle caps lock on key press only (not repeat or release)
    if keycode == 58 {
        if value == 1 {
            let prev = CAPS_LOCK_ACTIVE.load(Ordering::Relaxed);
            CAPS_LOCK_ACTIVE.store(!prev, Ordering::Relaxed);
        }
        return;
    }

    // Only process key presses and repeats (not releases)
    if value == 0 {
        return;
    }

    // Generate VT100 escape sequences for special keys
    if let Some(seq) = keycode_to_escape_seq(keycode) {
        for &b in seq {
            if !crate::tty::push_char_nonblock(b) {
                crate::ipc::stdin::push_byte_from_irq(b);
            }
        }
        return;
    }

    let shift = SHIFT_PRESSED.load(Ordering::Relaxed);
    let caps = CAPS_LOCK_ACTIVE.load(Ordering::Relaxed);
    let ctrl = CTRL_PRESSED.load(Ordering::Relaxed);

    let c = if ctrl {
        ctrl_char_from_keycode(keycode)
    } else {
        let effective_shift = if is_letter(keycode) { shift ^ caps } else { shift };
        keycode_to_char(keycode, effective_shift)
    };

    if let Some(c) = c {
        if !crate::tty::push_char_nonblock(c as u8) {
            crate::ipc::stdin::push_byte_from_irq(c as u8);
        }
    }
}

/// Process an absolute axis event (mouse/tablet movement).
fn process_abs_event(code: u16, value: u32) {
    match code {
        abs_code::ABS_X => {
            let (sw, _) = screen_dimensions();
            let x = (value as u64 * sw as u64 / (TABLET_ABS_MAX as u64 + 1)) as u32;
            MOUSE_X.store(x.min(sw.saturating_sub(1)), Ordering::Relaxed);
        }
        abs_code::ABS_Y => {
            let (_, sh) = screen_dimensions();
            let y = (value as u64 * sh as u64 / (TABLET_ABS_MAX as u64 + 1)) as u32;
            MOUSE_Y.store(y.min(sh.saturating_sub(1)), Ordering::Relaxed);
        }
        _ => {}
    }
}

// =============================================================================
// Public Query Functions
// =============================================================================

/// Get current mouse position in screen coordinates.
pub fn mouse_position() -> (u32, u32) {
    (MOUSE_X.load(Ordering::Relaxed), MOUSE_Y.load(Ordering::Relaxed))
}

/// Get current mouse position and button state.
pub fn mouse_state() -> (u32, u32, u32) {
    (
        MOUSE_X.load(Ordering::Relaxed),
        MOUSE_Y.load(Ordering::Relaxed),
        MOUSE_BUTTONS.load(Ordering::Relaxed),
    )
}
