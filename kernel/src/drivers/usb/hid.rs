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

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// =============================================================================
// Diagnostic counters (read by heartbeat in timer_interrupt.rs)
// =============================================================================

/// Counts keyboard reports where at least one byte is non-zero.
/// If this stays 0 while user types, the USB reports are empty.
pub static NONZERO_KBD_COUNT: AtomicU64 = AtomicU64::new(0);

/// Last keyboard report packed as a u64 (LE: byte[0] in LSB).
/// Allows heartbeat to display the most recent report bytes.
pub static LAST_KBD_REPORT_U64: AtomicU64 = AtomicU64::new(0);

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

/// Once we see the first absolute tablet report (6+ bytes), latch into tablet mode.
/// All subsequent reports are parsed as absolute, regardless of byte[1] value.
static IS_ABSOLUTE_TABLET: AtomicBool = AtomicBool::new(false);

/// Click stabilization: freeze position on button-down, require movement beyond
/// a dead zone before tracking resumes. Prevents Mac trackpad press-shift.
/// Stores the position at the moment button was pressed (raw abs coords, not screen).
static CLICK_FREEZE_X: AtomicU32 = AtomicU32::new(0);
static CLICK_FREEZE_Y: AtomicU32 = AtomicU32::new(0);
/// Set true on button-down, cleared once movement exceeds dead zone threshold.
static CLICK_FROZEN: AtomicBool = AtomicBool::new(false);
/// Dead zone in raw absolute coordinates (0-32767 range).
/// ~200 raw units = ~8 pixels at 1280 width. Absorbs trackpad press-shift.
const CLICK_DEAD_ZONE: u32 = 200;

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

    // Diagnostic: track report contents for heartbeat visibility
    let report_u64 = u64::from_le_bytes([
        report[0], report[1], report[2], report[3],
        report[4], report[5], report[6], report[7],
    ]);
    LAST_KBD_REPORT_U64.store(report_u64, Ordering::Relaxed);
    if report_u64 != 0 {
        NONZERO_KBD_COUNT.fetch_add(1, Ordering::Relaxed);
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
    crate::drivers::virtio::gpu_pci::dimensions()
        .or_else(|| crate::drivers::virtio::gpu_mmio::dimensions())
        .or_else(|| {
            crate::graphics::arm64_fb::FB_INFO_CACHE.get().map(|c| (c.width as u32, c.height as u32))
        })
        .unwrap_or((1280, 960))
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
/// Counter for diagnostic logging of first few mouse reports.
static MOUSE_LOG_COUNT: AtomicU64 = AtomicU64::new(0);

pub fn process_mouse_report(report: &[u8]) {
    if report.len() < 3 {
        return;
    }

    // Log first few non-zero mouse reports for debugging coordinate mapping
    let log_n = MOUSE_LOG_COUNT.load(Ordering::Relaxed);
    if log_n < 10 && report.iter().take(8).any(|&b| b != 0 && b != 0xDE) {
        MOUSE_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
        crate::serial_println!(
            "[mouse-report] #{}: [{:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}] len={}",
            log_n,
            report.get(0).copied().unwrap_or(0),
            report.get(1).copied().unwrap_or(0),
            report.get(2).copied().unwrap_or(0),
            report.get(3).copied().unwrap_or(0),
            report.get(4).copied().unwrap_or(0),
            report.get(5).copied().unwrap_or(0),
            report.get(6).copied().unwrap_or(0),
            report.get(7).copied().unwrap_or(0),
            report.len(),
        );
    }

    let buttons = report[0] as u32;
    let prev_buttons = MOUSE_BUTTONS.load(Ordering::Relaxed);
    MOUSE_BUTTONS.store(buttons, Ordering::Relaxed);

    // Log button state changes (press/release) to debug click-jump issues.
    // This fires regardless of MOUSE_LOG_COUNT to capture late button events.
    static BUTTON_LOG_COUNT: AtomicU64 = AtomicU64::new(0);
    if buttons != prev_buttons && BUTTON_LOG_COUNT.load(Ordering::Relaxed) < 20 {
        BUTTON_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
        crate::serial_println!(
            "[mouse-click] btn {} -> {}: [{:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}] len={} actual={}",
            prev_buttons, buttons,
            report.get(0).copied().unwrap_or(0),
            report.get(1).copied().unwrap_or(0),
            report.get(2).copied().unwrap_or(0),
            report.get(3).copied().unwrap_or(0),
            report.get(4).copied().unwrap_or(0),
            report.get(5).copied().unwrap_or(0),
            report.get(6).copied().unwrap_or(0),
            report.get(7).copied().unwrap_or(0),
            report.len(),
            {
                let mut len = report.len();
                if len > 8 { while len > 3 && report[len - 1] == 0xDE { len -= 1; } }
                len
            },
        );
    }

    let (sw, sh) = screen_dimensions();

    // Determine actual report length: the XHCI driver fills the 64-byte buffer
    // with 0xDE sentinel before DMA. Actual report bytes overwrite the sentinel.
    // Scan from the end to find where real data starts.
    let actual_len = if report.len() > 8 {
        let mut len = report.len();
        while len > 3 && report[len - 1] == 0xDE {
            len -= 1;
        }
        len
    } else {
        report.len()
    };

    // Absolute tablet detection: once we see the first 6+ byte absolute report,
    // latch into tablet mode. This prevents intermittent fallthrough to the relative
    // path during drag when report[1] may be non-zero (e.g., report ID byte).
    let is_tablet = IS_ABSOLUTE_TABLET.load(Ordering::Relaxed);
    if !is_tablet && actual_len >= 6 && report[1] == 0 {
        IS_ABSOLUTE_TABLET.store(true, Ordering::Relaxed);
        crate::serial_println!("[mouse] Latched into absolute tablet mode (first 6-byte report with byte[1]=0)");
    }

    // Absolute tablet path: use absolute coordinates from bytes 2-5.
    // Once latched, always use this path for 6+ byte reports regardless of byte[1].
    if IS_ABSOLUTE_TABLET.load(Ordering::Relaxed) && actual_len >= 6 {
        let abs_x = u16::from_le_bytes([report[2], report[3]]) as u32;
        let abs_y = u16::from_le_bytes([report[4], report[5]]) as u32;

        // Click stabilization: on button-down, freeze position until movement
        // exceeds a dead zone. This absorbs Mac trackpad press-shift (~10-20px).
        if buttons != 0 && prev_buttons == 0 {
            // Button just pressed — freeze at current raw abs position
            CLICK_FREEZE_X.store(abs_x, Ordering::Relaxed);
            CLICK_FREEZE_Y.store(abs_y, Ordering::Relaxed);
            CLICK_FROZEN.store(true, Ordering::Relaxed);
            return; // Don't update position on the press event itself
        }
        if buttons == 0 && prev_buttons != 0 {
            // Button released — unfreeze
            CLICK_FROZEN.store(false, Ordering::Relaxed);
        }
        if CLICK_FROZEN.load(Ordering::Relaxed) {
            // Still in dead zone — check if movement exceeds threshold
            let freeze_x = CLICK_FREEZE_X.load(Ordering::Relaxed);
            let freeze_y = CLICK_FREEZE_Y.load(Ordering::Relaxed);
            let dx = (abs_x as i32 - freeze_x as i32).unsigned_abs();
            let dy = (abs_y as i32 - freeze_y as i32).unsigned_abs();
            if dx < CLICK_DEAD_ZONE && dy < CLICK_DEAD_ZONE {
                return; // Still within dead zone, don't move cursor
            }
            // Exceeded dead zone — unfreeze and allow movement
            CLICK_FROZEN.store(false, Ordering::Relaxed);
        }

        let new_x = (abs_x * sw / 32768).min(sw - 1);
        let new_y = (abs_y * sh / 32768).min(sh - 1);
        if log_n < 10 {
            crate::serial_println!(
                "[mouse-pos] abs=({},{}) screen={}x{} div=32768 -> ({},{})",
                abs_x, abs_y, sw, sh, new_x, new_y
            );
        }
        MOUSE_X.store(new_x, Ordering::Relaxed);
        MOUSE_Y.store(new_y, Ordering::Relaxed);
        return;
    }

    // Boot protocol relative mouse: 3-4 byte reports
    // Format: [buttons, dx (i8), dy (i8), wheel (i8)]
    let dx = report[1] as i8 as i32;
    let dy = report[2] as i8 as i32;

    let old_x = MOUSE_X.load(Ordering::Relaxed) as i32;
    let new_x = (old_x + dx).clamp(0, sw as i32 - 1) as u32;
    MOUSE_X.store(new_x, Ordering::Relaxed);

    let old_y = MOUSE_Y.load(Ordering::Relaxed) as i32;
    let new_y = (old_y + dy).clamp(0, sh as i32 - 1) as u32;
    MOUSE_Y.store(new_y, Ordering::Relaxed);
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
