//! USB HID boot keyboard for the x86-metal console.
//!
//! Source authority: HID 1.11 (boot protocol, class requests, Appendices B,
//! C and F), the HID Usage Tables' Keyboard/Keypad page and USB 2.0 chapter
//! 9. The xHCI worker (`drivers::xhci`) configures each keyboard's interrupt-IN
//! endpoint and hands its reports here: `report` turns them into key events,
//! which go to the input core (`crate::input`), and the console's lock state
//! comes back for the keyboards' LEDs.
//! The pure modules' host tests: `rustc --edition=2021 --test <file>.rs`.

pub mod interface;
pub mod report;

/// Where boot keyboard input meets the input core.
pub struct Console {
    /// The console target has started and its replug exercise is over; the
    /// one-shot first-key lines report only keys pressed after that.
    armed: bool,
    first_report: bool,
    first_typed: bool,
}

impl Console {
    pub fn new() -> Self {
        Self {
            armed: false,
            first_report: false,
            first_typed: false,
        }
    }

    /// Arm the first-key lines, and TTY 0's first-USB-byte line with them.
    pub fn arm(&mut self) {
        self.armed = true;
        if let Some(tty) = crate::tty::console() {
            tty.arm_first_usb();
        }
    }

    pub fn note_report(&mut self, summary: &report::Summary) {
        if self.armed && summary.nonempty && !self.first_report {
            self.first_report = true;
            log::info!("[hid] first key report");
        }
    }

    /// One key event for the input core.
    pub fn event(&mut self, code: u16, value: i32) {
        let sent = crate::input::report_key(crate::input::Device::UsbBootKeyboard, code, value);
        if sent == 0 {
            return;
        }
        // Report from the xHCI worker after TTY admission.
        if let Some(tty) = crate::tty::console() {
            tty.report_first_usb();
        }
        if self.armed && !self.first_typed {
            self.first_typed = true;
            log::info!("[hid] typed into the console tty: {} bytes", sent);
        }
    }

    /// The keyboards' LED report for the console's lock state.
    pub fn leds(&self) -> u8 {
        crate::console::keys::leds()
    }
}
