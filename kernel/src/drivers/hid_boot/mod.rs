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
pub mod pointer;
pub mod report;

/// Nonempty USB boot-keyboard reports observed by the production completion path.
pub static NONZERO_KBD_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Where boot keyboard input meets the input core.
pub struct Console {
    /// The console target has started and its replug exercise is over; the
    /// one-shot first-key lines report only keys pressed after that.
    armed: bool,
    first_report: bool,
}

impl Console {
    pub fn new() -> Self {
        Self {
            armed: false,
            first_report: false,
        }
    }

    /// Arm the first-key lines, and TTY 0's first-USB-byte line with them.
    pub fn arm(&mut self) {
        self.armed = true;
        CONSOLE_ARMED.store(true, core::sync::atomic::Ordering::Release);
        if let Some(tty) = crate::tty::console() {
            tty.arm_first_usb();
        }
    }

    pub fn note_report(&mut self, summary: &report::Summary) {
        if summary.nonempty {
            NONZERO_KBD_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        if self.armed && summary.nonempty && !self.first_report {
            self.first_report = true;
            log::info!("[hid] first key report");
        }
    }

    /// The keyboards' LED report for the console's lock state.
    pub fn leds(&self) -> u8 {
        crate::console::keys::leds()
    }
}

static CONSOLE_ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static FIRST_TYPED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static LAST_LEDS: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// Called by the task-context input service after actual console TTY admission.
pub fn console_delivery(sent: usize) {
    use core::sync::atomic::Ordering;
    // Console routing now runs after the xHCI worker's completion dispatch.
    // Wake that worker when a lock key changes, even if no TTY byte was sent.
    let leds = crate::console::keys::leds();
    if LAST_LEDS.swap(leds, Ordering::AcqRel) != leds {
        crate::drivers::xhci::lock_state_changed();
    }
    if sent == 0 {
        return;
    }
    if let Some(tty) = crate::tty::console() {
        tty.report_first_usb();
    }
    if CONSOLE_ARMED.load(Ordering::Acquire) && !FIRST_TYPED.swap(true, Ordering::AcqRel) {
        log::info!("[hid] typed into the console tty: {sent} bytes");
    }
}
