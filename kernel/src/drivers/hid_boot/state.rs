//! The pointer and keyboard state consumed by the existing graphics syscalls.
//! Report decoding runs in the USB worker; readers take atomic snapshots.
use super::pointer::Motion;
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering};

// 24-bit x, 24-bit y, 8 button bits. One atomic snapshot keeps coordinates and
// buttons from different reports from being mixed by a polling reader.
static BUTTONS: [AtomicU8; 256] = [const { AtomicU8::new(0) }; 256];
static POINTER: AtomicU64 = AtomicU64::new(0);
static PRESSES: AtomicU32 = AtomicU32::new(0);
static WHEEL: AtomicI32 = AtomicI32::new(0);

pub fn report(port: u8, motion: Motion) {
    let Some(info) = crate::graphics::arm64_fb::FB_INFO_CACHE.get() else {
        return;
    };
    let (width, height) = (info.width, info.height);
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
