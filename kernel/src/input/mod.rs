//! The input core, first slice (GUI plan section 5.1; `docs/console/contract.md`
//! section 9).
//!
//! Keyboard drivers report key events here and do no translation of their own:
//! a key code from the published `input-event-codes.h` and a value, 0 release,
//! 1 press, 2 repeat. The core derives the modifier state from those events,
//! the same way for each driver, and routes each event to its consumer. The
//! console keymap (`console::keys`) is the one consumer so far; the
//! window-manager effort adds the event ABI, `/dev/uinput`, grabs and the
//! compositor consumer here.
//!
//! `report_key` runs in interrupt handlers (PS/2, virtio-input) and in the xHCI
//! worker thread: it waits only on locks whose holders mask interrupts, and only
//! tries the others; it does not allocate and does not log.

pub mod event;
#[cfg(target_arch = "aarch64")]
pub mod pointer;

use core::sync::atomic::{AtomicU16, AtomicU64, AtomicU8, Ordering};
use event::Modifiers;

/// The keyboards that report here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    /// The i8042 keyboard (x86-64).
    Ps2,
    /// `virtio-keyboard-device` over MMIO (ARM64 QEMU).
    VirtioKeyboard,
    /// A USB HID boot keyboard on the clean-room xHCI (x86-metal).
    UsbBootKeyboard(u8),
}

// USB root ports are numbered 1..=255 by xHCI. Keep their identities stable on detach.
const DEVICES: usize = 258;

impl Device {
    pub(crate) fn index(self) -> usize {
        match self {
            Device::Ps2 => 0,
            Device::VirtioKeyboard => 1,
            Device::UsbBootKeyboard(port) => 2 + port as usize,
        }
    }

    /// The source the console terminal records for the bytes it gets.
    fn source(self) -> crate::tty::InputSource {
        match self {
            Device::Ps2 => crate::tty::InputSource::Ps2,
            Device::VirtioKeyboard => crate::tty::InputSource::Other,
            Device::UsbBootKeyboard(_) => crate::tty::InputSource::UsbKeyboard,
        }
    }
}

/// Each device's held modifier keys. The state the consumer sees is their union.
static HELD: [AtomicU8; DEVICES] = [const { AtomicU8::new(0) }; DEVICES];
/// Function keys whose press was a reserved chord, until their release.
static RESERVED: [AtomicU16; DEVICES] = [const { AtomicU16::new(0) }; DEVICES];

static SUPER_TAPS: AtomicU64 = AtomicU64::new(0);

/// The same derived modifier state used by the console, projected to the graphics ABI.
pub fn poll_modifier_state() -> u8 {
    let modifiers = Modifiers(
        HELD.iter()
            .fold(0, |all, held| all | held.load(Ordering::Relaxed)),
    );
    modifiers.shift() as u8
        | (modifiers.ctrl() as u8) << 1
        | (modifiers.alt() as u8) << 2
        | (modifiers.meta() as u8) << 3
}
pub fn has_pending_super_tap() -> bool {
    SUPER_TAPS.load(Ordering::Acquire) != 0
}
pub fn take_super_tap_count() -> u64 {
    SUPER_TAPS.swap(0, Ordering::AcqRel)
}

/// One key event from `device`. Returns how many bytes it sent the foreground terminal.
pub fn report_key(device: Device, code: u16, value: i32) -> usize {
    // One device reports from one context at a time, so its own word needs no read-modify-write.
    let slot = &HELD[device.index()];
    let previous = slot.load(Ordering::Relaxed);
    let next = event::update_modifiers(previous, code, value);
    slot.store(next, Ordering::Relaxed);
    if !Modifiers(previous).meta() && Modifiers(next).meta() {
        SUPER_TAPS.fetch_add(1, Ordering::Release);
        // The compositor is ready while a tap is latched. Bytes wake it where they are
        // admitted to its stdin (`ipc::stdin`), including those a busy TTY admits later.
        #[cfg(target_arch = "aarch64")]
        crate::syscall::graphics::wake_compositor_for_input();
    }
    let modifiers = Modifiers(
        HELD.iter()
            .fold(0, |all, held| all | held.load(Ordering::Relaxed)),
    );

    // The routing point (contract section 9.2). The reserved chords, Ctrl+Alt+F1 to F12 and
    // their releases, reach the console under any grab; the window-manager effort's
    // compositor grab takes the other events while its terminal is in the foreground.
    let mut reserved = false;
    let _ =
        RESERVED[device.index()].fetch_update(Ordering::AcqRel, Ordering::Acquire, |mut held| {
            reserved = event::reserved(&mut held, code, value, modifiers);
            Some(held)
        });
    crate::console::keys::key(
        device.index(),
        code,
        value,
        modifiers,
        reserved,
        device.source(),
    )
}
