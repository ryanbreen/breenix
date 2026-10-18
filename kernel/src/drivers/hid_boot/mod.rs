//! USB HID boot keyboard for the x86-metal console.
//!
//! Source authority: HID 1.11 (boot protocol, class requests, Appendices B,
//! C and F), the HID Usage Tables' Keyboard/Keypad page and USB 2.0 chapter
//! 9. The xHCI worker (`drivers::xhci`) configures each keyboard's interrupt-IN
//! endpoint and hands its reports here: `report` turns them into console
//! bytes and chords, `queue` keeps the bytes until TTY 0 accepts them.
//! The pure modules' host tests: `rustc --edition=2021 --test <file>.rs`.

pub mod interface;
pub mod queue;
pub mod report;

use alloc::boxed::Box;

/// Bytes waiting for TTY 0: minutes of typing behind a held lock.
const QUEUE_BYTES: usize = 4096;
/// How soon the worker offers retained bytes to TTY 0 again.
pub const RETRY_NS: u64 = 5_000_000;

/// Where boot keyboard input meets the console TTY.
pub struct Console {
    queue: Box<queue::Retained<QUEUE_BYTES>>,
    first_report: bool,
    first_typed: bool,
}

impl Console {
    pub fn new() -> Self {
        Self {
            queue: Box::new(queue::Retained::new()),
            first_report: false,
            first_typed: false,
        }
    }

    pub fn note_report(&mut self, summary: &report::Summary) {
        if summary.nonempty && !self.first_report {
            self.first_report = true;
            log::info!("[hid] first key report");
        }
    }

    pub fn output(&mut self, output: report::Output) {
        match output {
            report::Output::Bytes(bytes) => {
                if !self.queue.push(bytes.as_slice()) && self.queue.refused() == 1 {
                    log::warn!("[hid] console input queue full; key events are being refused");
                }
            }
            // Kept distinct for the virtual-terminal layer, which does not exist yet.
            report::Output::Chord(chord) => {
                log::debug!("[hid] console chord {:?}: no virtual terminals yet", chord);
            }
        }
    }

    /// Bytes are waiting for TTY 0.
    pub fn pending(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Offer retained bytes to TTY 0. True while some remain queued.
    pub fn flush(&mut self) -> bool {
        let taken = self.queue.drain(&mut crate::tty::push_char_nonblock);
        if taken != 0 && !self.first_typed {
            self.first_typed = true;
            log::info!("[hid] typed into the console tty: {} bytes", taken);
        }
        !self.queue.is_empty()
    }
}
