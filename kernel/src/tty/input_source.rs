//! Source of each byte admitted to the console line discipline. No byte values
//! or counts are retained here, including while terminal echo is disabled.
//! The USB keyboard worker tags its bytes `UsbKeyboard` and the PS/2 keyboard
//! task tags its bytes `Ps2`; the other entries, including the x86 PS/2
//! interrupt handler and virtio input, record `Other`.
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// `first_usb` states: the first-USB-byte report is armed once the console
/// target has started, so a key pressed earlier is not reported as first.
const DISARMED: u8 = 0;
const ARMED: u8 = 1;
const SEEN: u8 = 2;
const REPORTED: u8 = 3;

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum InputSource {
    UsbKeyboard = 1,
    Ps2 = 2,
    Serial = 4,
    Other = 8,
}

pub struct InputSources {
    sources: AtomicU8,
    authenticated: AtomicBool,
    first_usb: AtomicU8,
}

impl InputSources {
    pub const fn new() -> Self {
        Self { sources: AtomicU8::new(0), authenticated: AtomicBool::new(false), first_usb: AtomicU8::new(DISARMED) }
    }

    /// Called only after the input lock is acquired, before exposing the byte.
    pub fn record(&self, source: InputSource) {
        self.sources.fetch_or(source as u8, Ordering::Relaxed);
        if matches!(source, InputSource::UsbKeyboard) {
            let _ = self.first_usb.compare_exchange(ARMED, SEEN, Ordering::Release, Ordering::Relaxed);
        }
    }

    /// Start watching for the first USB keyboard byte. Only the first call arms it.
    pub fn arm_first_usb(&self) {
        let _ = self.first_usb.compare_exchange(DISARMED, ARMED, Ordering::AcqRel, Ordering::Relaxed);
    }

    pub fn take_first_usb(&self) -> bool {
        self.first_usb.compare_exchange(SEEN, REPORTED, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    pub fn keyboard_only(&self) -> bool {
        self.sources.load(Ordering::Relaxed) == InputSource::UsbKeyboard as u8
    }

    /// Serialized with ingress by the TTY input lock; pending input is flushed.
    pub fn begin(&self) -> Option<bool> {
        let ended = self.authenticated.swap(false, Ordering::Relaxed).then(|| self.keyboard_only());
        self.sources.store(0, Ordering::Relaxed);
        ended
    }

    pub fn authenticate(&self, accepted: bool) -> bool {
        self.authenticated.store(accepted, Ordering::Relaxed);
        self.keyboard_only()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_serial_ps2_and_mixed_attempts_cannot_claim_usb() {
        for source in [InputSource::Serial, InputSource::Ps2, InputSource::Other] {
            let state = InputSources::new();
            assert!(!state.authenticate(true));
            state.begin();
            state.record(InputSource::UsbKeyboard);
            assert!(state.authenticate(false));
            state.record(source);
            assert!(!state.authenticate(true));
            assert_eq!(state.begin(), Some(false));
            assert!(!state.keyboard_only());
        }
    }

    #[test]
    fn first_usb_is_one_shot_and_session_includes_every_later_byte() {
        let state = InputSources::new();
        assert!(!state.take_first_usb());
        state.record(InputSource::UsbKeyboard); // Before the console target: not reported.
        assert!(!state.take_first_usb());
        state.arm_first_usb();
        state.arm_first_usb();
        assert!(!state.take_first_usb());
        state.record(InputSource::UsbKeyboard);
        assert!(state.take_first_usb());
        state.arm_first_usb(); // Arming again cannot repeat the report.
        state.record(InputSource::UsbKeyboard);
        assert!(!state.take_first_usb());
        assert!(state.authenticate(true));
        state.record(InputSource::UsbKeyboard);
        assert!(!state.take_first_usb());
        assert_eq!(state.begin(), Some(true));
        state.record(InputSource::UsbKeyboard);
        assert!(!state.take_first_usb());
        assert!(state.authenticate(true));
        state.record(InputSource::Serial); // Even erased/control input contaminates the proof.
        assert_eq!(state.begin(), Some(false));
    }
}
