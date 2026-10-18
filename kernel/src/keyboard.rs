//! The PS/2 keyboard (i8042), x86-64: its interrupt handler hands each byte
//! here, and the set-1 decoder turns it into key events for the input core.

use spin::Mutex;

mod set1;

/// The keyboard's set-1 decoder. Only its interrupt handler takes it, after
/// `init`.
static DECODER: Mutex<set1::Decoder> = Mutex::new(set1::Decoder::new());

/// Reset the decoder, before the keyboard interrupt is enabled.
pub fn init() {
    *DECODER.lock() = set1::Decoder::new();
}

/// A byte the PS/2 keyboard sent (port 0x60), from its interrupt handler:
/// the key event it completes goes to the input core.
pub fn on_scancode(byte: u8) {
    let event = DECODER.try_lock().and_then(|mut decoder| decoder.byte(byte));
    if let Some((code, value)) = event {
        crate::input::report_key(crate::input::Device::Ps2, code, value);
    }
}
