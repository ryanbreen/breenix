//! The pointer state the graphics syscalls and the cursor read. The USB HID
//! pointer driver decodes reports in the xHCI worker and reports motion here;
//! readers take atomic snapshots.
use crate::drivers::hid_boot::pointer::Motion;
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering};

// 24-bit x, 24-bit y, 8 button bits. One atomic snapshot keeps coordinates and
// buttons from different reports from being mixed by a polling reader.
static BUTTONS: [AtomicU8; 256] = [const { AtomicU8::new(0) }; 256];
static POINTER: AtomicU64 = AtomicU64::new(0);
static PRESSES: AtomicU32 = AtomicU32::new(0);
static WHEEL: AtomicI32 = AtomicI32::new(0);
// The screen the pointer moves over, `width << 32 | height`, once the GPU core's screen
// replaces the firmware framebuffer (`set_screen`); 0 until then.
static SCREEN: AtomicU64 = AtomicU64::new(0);

/// Scale the pointer to a `width` x `height` screen from now on.
pub fn set_screen(width: u32, height: u32) {
    SCREEN.store((width as u64) << 32 | height as u64, Ordering::Release);
}

/// The screen `set_screen` gave, if any.
pub fn screen() -> Option<(u32, u32)> {
    match SCREEN.load(Ordering::Acquire) {
        0 => None,
        screen => Some(((screen >> 32) as u32, screen as u32)),
    }
}

pub fn report(port: u8, motion: Motion) {
    let (width, height) = match screen() {
        Some((width, height)) => (width as usize, height as usize),
        None => match crate::graphics::arm64_fb::FB_INFO_CACHE.get() {
            Some(info) => (info.width, info.height),
            None => return,
        },
    };
    let old = POINTER.load(Ordering::Acquire);
    let (x, y, buttons) = unpack(old);
    let x = motion.x.map_or(x, |axis| axis.position(x, width as u32));
    let y = motion.y.map_or(y, |axis| axis.position(y, height as u32));
    if let Some(value) = motion.buttons {
        BUTTONS[port as usize].store(value as u8, Ordering::Release);
    }
    let new_buttons = BUTTONS
        .iter()
        .fold(0u32, |all, held| all | held.load(Ordering::Acquire) as u32);
    PRESSES.fetch_or(new_buttons & !buttons, Ordering::AcqRel);
    WHEEL.fetch_add(motion.wheel, Ordering::AcqRel);
    POINTER.store(
        x as u64 | (y as u64) << 24 | (new_buttons as u64) << 48,
        Ordering::Release,
    );
    crate::syscall::graphics::wake_compositor_if_waiting();
}

fn unpack(value: u64) -> (u32, u32, u32) {
    (
        (value & 0xff_ffff) as u32,
        ((value >> 24) & 0xff_ffff) as u32,
        ((value >> 48) & 255) as u32,
    )
}

pub fn mouse_state() -> (u32, u32, u32) {
    unpack(POINTER.load(Ordering::Acquire))
}
pub fn mouse_position() -> (u32, u32) {
    let (x, y, _) = mouse_state();
    (x, y)
}
pub fn mouse_state_consume() -> (u32, u32, u32) {
    let pressed = PRESSES.swap(0, Ordering::AcqRel);
    let (x, y, buttons) = mouse_state();
    (x, y, buttons | pressed)
}
pub fn mouse_wheel_consume() -> i32 {
    WHEEL.swap(0, Ordering::AcqRel)
}
pub fn has_pending_press() -> bool {
    PRESSES.load(Ordering::Acquire) != 0
}
pub fn has_pending_scroll() -> bool {
    WHEEL.load(Ordering::Acquire) != 0
}

/// Release a detached pointer's buttons without releasing another device's.
pub fn release(port: u8) {
    BUTTONS[port as usize].store(0, Ordering::Release);
    report(port, Motion::default());
}
