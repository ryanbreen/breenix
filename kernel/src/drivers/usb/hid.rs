//! USB HID Class Driver (Boot Protocol Keyboard + Mouse)
//!
//! Processes boot protocol HID reports from USB keyboard and mouse devices
//! attached via the XHCI host controller. Routes keyboard events to the
//! TTY subsystem and mouse events to the input atomics, using the same
//! paths as the VirtIO input driver.
//!
//! Boot protocol gives fixed-format reports:
//! - Keyboard: 8 bytes (1 modifier + 1 reserved + 6 keycodes)
//! - Mouse: 3-4 bytes (1 buttons + 1 dx + 1 dy + optional wheel)

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// =============================================================================
// State tracking
// =============================================================================

/// Previous keyboard report for detecting key press/release transitions.
static mut PREV_KBD_REPORT: [u8; 8] = [0; 8];

/// Modifier key state (tracked from HID modifier byte).
static SHIFT_PRESSED: AtomicBool = AtomicBool::new(false);
static CTRL_PRESSED: AtomicBool = AtomicBool::new(false);
static CAPS_LOCK_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Mouse position in screen coordinates (shared with VirtIO input atomics).
/// These are the authoritative mouse position for the entire system.
static MOUSE_X: AtomicU32 = AtomicU32::new(0);
static MOUSE_Y: AtomicU32 = AtomicU32::new(0);
static MOUSE_BUTTONS: AtomicU32 = AtomicU32::new(0);

// =============================================================================
// USB HID Usage ID → Linux Keycode mapping
// =============================================================================

/// Map USB HID keyboard usage IDs (USB HID Usage Tables, Section 10) to
/// Linux keycodes (same codes used by input_mmio.rs).
///
/// USB HID usage IDs: 0x04='a' through 0x1D='z', 0x1E='1' through 0x27='0',
/// 0x28=Enter, 0x29=Escape, 0x2A=Backspace, 0x2B=Tab, 0x2C=Space, etc.
///
/// Returns a Linux keycode or 0 if unmapped.
fn hid_usage_to_linux_keycode(usage: u8) -> u16 {
    match usage {
        // Letters: USB 0x04-0x1D → Linux keycodes for QWERTY layout
        0x04 => 30,  // a
        0x05 => 48,  // b
        0x06 => 46,  // c
        0x07 => 32,  // d
        0x08 => 18,  // e
        0x09 => 33,  // f
        0x0A => 34,  // g
        0x0B => 35,  // h
        0x0C => 23,  // i
        0x0D => 36,  // j
        0x0E => 37,  // k
        0x0F => 38,  // l
        0x10 => 50,  // m
        0x11 => 49,  // n
        0x12 => 24,  // o
        0x13 => 25,  // p
        0x14 => 16,  // q
        0x15 => 19,  // r
        0x16 => 31,  // s
        0x17 => 20,  // t
        0x18 => 22,  // u
        0x19 => 47,  // v
        0x1A => 17,  // w
        0x1B => 45,  // x
        0x1C => 21,  // y
        0x1D => 44,  // z

        // Numbers: USB 0x1E-0x27 → Linux keycodes 2-11
        0x1E => 2,   // 1
        0x1F => 3,   // 2
        0x20 => 4,   // 3
        0x21 => 5,   // 4
        0x22 => 6,   // 5
        0x23 => 7,   // 6
        0x24 => 8,   // 7
        0x25 => 9,   // 8
        0x26 => 10,  // 9
        0x27 => 11,  // 0

        // Special keys
        0x28 => 28,  // Enter
        0x29 => 1,   // Escape
        0x2A => 14,  // Backspace
        0x2B => 15,  // Tab
        0x2C => 57,  // Space
        0x2D => 12,  // - (minus)
        0x2E => 13,  // = (equals)
        0x2F => 26,  // [ (left bracket)
        0x30 => 27,  // ] (right bracket)
        0x31 => 43,  // \ (backslash)
        0x33 => 39,  // ; (semicolon)
        0x34 => 40,  // ' (apostrophe)
        0x35 => 41,  // ` (grave accent)
        0x36 => 51,  // , (comma)
        0x37 => 52,  // . (period)
        0x38 => 53,  // / (slash)
        0x39 => 58,  // Caps Lock

        // Function keys
        0x3A => 59,  // F1
        0x3B => 60,  // F2
        0x3C => 61,  // F3
        0x3D => 62,  // F4
        0x3E => 63,  // F5
        0x3F => 64,  // F6
        0x40 => 65,  // F7
        0x41 => 66,  // F8
        0x42 => 67,  // F9
        0x43 => 68,  // F10

        // Navigation keys
        0x4F => 106, // Right Arrow
        0x50 => 105, // Left Arrow
        0x51 => 108, // Down Arrow
        0x52 => 103, // Up Arrow
        0x4A => 102, // Home
        0x4B => 104, // Page Up
        0x4C => 111, // Delete
        0x4D => 107, // End
        0x4E => 109, // Page Down

        _ => 0, // Unmapped
    }
}

// =============================================================================
// Keyboard Report Processing
// =============================================================================

/// Process a USB boot protocol keyboard report (8 bytes).
///
/// Report format:
/// - Byte 0: Modifier flags (LCtrl, LShift, LAlt, LGui, RCtrl, RShift, RAlt, RGui)
/// - Byte 1: Reserved
/// - Bytes 2-7: Up to 6 simultaneous key usage IDs (0 = no key)
///
/// Compares with previous report to detect key press/release transitions.
pub fn process_keyboard_report(report: &[u8]) {
    if report.len() < 8 {
        return;
    }

    let modifiers = report[0];
    let keys = &report[2..8];

    // Update modifier state from the modifier byte
    // Bit 0: Left Ctrl,  Bit 1: Left Shift,  Bit 4: Right Ctrl, Bit 5: Right Shift
    let shift = (modifiers & 0x02) != 0 || (modifiers & 0x20) != 0;
    let ctrl = (modifiers & 0x01) != 0 || (modifiers & 0x10) != 0;
    SHIFT_PRESSED.store(shift, Ordering::Relaxed);
    CTRL_PRESSED.store(ctrl, Ordering::Relaxed);

    let prev = unsafe { &*(&raw const PREV_KBD_REPORT) };

    // Detect newly pressed keys (in current but not in previous)
    for &usage in keys {
        if usage == 0 || usage == 1 { continue; } // 0=no key, 1=rollover error

        // Check if this key was already pressed in the previous report
        let was_pressed = prev[2..8].contains(&usage);
        if was_pressed { continue; } // Key held, not newly pressed

        // Handle Caps Lock toggle on press
        if usage == 0x39 {
            let prev_caps = CAPS_LOCK_ACTIVE.load(Ordering::Relaxed);
            CAPS_LOCK_ACTIVE.store(!prev_caps, Ordering::Relaxed);
            continue;
        }

        // Convert USB HID usage to Linux keycode
        let keycode = hid_usage_to_linux_keycode(usage);
        if keycode == 0 { continue; }

        inject_keycode(keycode, shift, ctrl);
    }

    // Save current report for next comparison
    unsafe {
        let dst = &raw mut PREV_KBD_REPORT;
        (*dst).copy_from_slice(&report[..8]);
    }
}

/// Inject a Linux keycode into the TTY input path.
///
/// Reuses the same keycode-to-character conversion as VirtIO input
/// (`input_mmio.rs`) for consistent behavior across input backends.
fn inject_keycode(keycode: u16, shift: bool, ctrl: bool) {
    // Generate VT100 escape sequences for special keys
    if let Some(seq) = crate::drivers::virtio::input_mmio::keycode_to_escape_seq(keycode) {
        for &b in seq {
            if !crate::tty::push_char_nonblock(b) {
                crate::ipc::stdin::push_byte_from_irq(b);
            }
        }
        return;
    }

    let caps = CAPS_LOCK_ACTIVE.load(Ordering::Relaxed);

    // Ctrl+letter → control character
    let c = if ctrl {
        crate::drivers::virtio::input_mmio::ctrl_char_from_keycode(keycode)
    } else {
        let effective_shift = if crate::drivers::virtio::input_mmio::is_letter(keycode) {
            shift ^ caps
        } else {
            shift
        };
        crate::drivers::virtio::input_mmio::keycode_to_char(keycode, effective_shift)
    };

    if let Some(c) = c {
        if !crate::tty::push_char_nonblock(c as u8) {
            crate::ipc::stdin::push_byte_from_irq(c as u8);
        }
    }
}

// =============================================================================
// Mouse Report Processing
// =============================================================================

/// Get current screen dimensions for mouse clamping.
fn screen_dimensions() -> (u32, u32) {
    crate::drivers::virtio::gpu_mmio::dimensions()
        .or_else(|| {
            crate::graphics::arm64_fb::FB_INFO_CACHE.get().map(|c| (c.width as u32, c.height as u32))
        })
        .unwrap_or((1280, 800))
}

/// Process a USB boot protocol mouse report (3-4 bytes).
///
/// Report format:
/// - Byte 0: Button flags (bit 0=left, bit 1=right, bit 2=middle)
/// - Byte 1: X displacement (signed i8)
/// - Byte 2: Y displacement (signed i8)
/// - Byte 3: Wheel displacement (optional, signed i8)
///
/// Updates the global mouse position atomics with clamping to screen bounds.
pub fn process_mouse_report(report: &[u8]) {
    if report.len() < 3 {
        return;
    }

    let buttons = report[0] as u32;
    let dx = report[1] as i8 as i32;
    let dy = report[2] as i8 as i32;

    MOUSE_BUTTONS.store(buttons, Ordering::Relaxed);

    let (sw, sh) = screen_dimensions();

    // Update X position with clamping
    let old_x = MOUSE_X.load(Ordering::Relaxed) as i32;
    let new_x = (old_x + dx).clamp(0, sw as i32 - 1) as u32;
    MOUSE_X.store(new_x, Ordering::Relaxed);

    // Update Y position with clamping
    let old_y = MOUSE_Y.load(Ordering::Relaxed) as i32;
    let new_y = (old_y + dy).clamp(0, sh as i32 - 1) as u32;
    MOUSE_Y.store(new_y, Ordering::Relaxed);

    // Mouse click dispatch could be added here for terminal tab switching
}

// =============================================================================
// Public Accessors
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
