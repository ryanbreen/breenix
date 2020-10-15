use conquer_once::spin::OnceCell;
use core::{
    pin::Pin,
    task::{Context, Poll},
};
use crossbeam_queue::ArrayQueue;

use futures_util::stream::{Stream, StreamExt};
use futures_util::task::AtomicWaker;

use lazy_static::lazy_static;
use spin::Mutex;

use crate::io::Port;

use crate::state;

use crate::constants::keyboard::{Key, KEYS, PORT};

use crate::event::EventType;

use crate::event::keyboard::{ControlKeyState, KeyEvent};

use crate::println;

static SCANCODE_QUEUE: OnceCell<ArrayQueue<u8>> = OnceCell::uninit();

static WAKER: AtomicWaker = AtomicWaker::new();

impl KeyEvent {
    const fn new(scancode: u8, character: char, modifiers: &Modifiers) -> KeyEvent {
        KeyEvent {
            event_type: EventType::KeyEvent,
            scancode: scancode,
            character: character,
            controls: ControlKeyState {
                cmd: modifiers.l_cmd || modifiers.r_cmd,
                ctrl: modifiers.l_ctrl,
                alt: modifiers.l_alt || modifiers.r_alt,
                shift: modifiers.l_shift || modifiers.r_shift,
                caps_lock: modifiers.caps_lock,
                scroll_lock: false,
                num_lock: false,
            },
        }
    }
}

/// Our keyboard state, including our I/O port, our currently pressed
/// modifiers, etc.
struct KeyState {
    /// The PS/2 serial IO port for the keyboard.  There's a huge amount of
    /// emulation going on at the hardware level to allow us to pretend to
    /// be an early-80s IBM PC.
    ///
    /// We could read the standard keyboard port directly using
    /// `inb(0x60)`, but it's nicer if we wrap it up in a `Port` object.
    port: Port<u8>,

    /// The collection of currently-pressed modifier keys.
    modifiers: Modifiers,
}

#[allow(dead_code)]
struct Modifiers {
    l_ctrl: bool,
    l_shift: bool,
    r_shift: bool,
    caps_lock: bool,
    l_cmd: bool,
    r_cmd: bool,
    l_alt: bool,
    r_alt: bool,
    last_key: u8,
}

impl Modifiers {
    const fn new() -> Modifiers {
        Modifiers {
            l_ctrl: false,
            l_shift: false,
            r_shift: false,
            caps_lock: false,
            l_cmd: false,
            r_cmd: false,
            l_alt: false,
            r_alt: false,
            last_key: 0,
        }
    }

    #[allow(dead_code)]
    fn cmd(&self) -> bool {
        self.l_cmd || self.r_cmd
    }

    fn update(&mut self, scancode: u8) {
        // printk!("{:x} {:x}", self.last_key, scancode);

        match scancode {
            0x5B => self.l_cmd = true,
            0xDB => self.l_cmd = false,
            0x5C => self.r_cmd = true,
            0xDC => self.r_cmd = false,
            0x2A => self.l_shift = true,
            0xAA => self.l_shift = false,
            0x36 => self.r_shift = true,
            0xB6 => self.r_shift = false,
            0x1D => self.l_ctrl = true,
            0x9D => self.l_ctrl = false,
            0x3A => self.caps_lock = !self.caps_lock,
            _ => {}
        }

        self.last_key = scancode;
    }

    fn apply_to(&self, key: Key) -> Option<char> {
        // Only alphabetic keys honor caps lock, so first distinguish between
        // alphabetic and non alphabetic keys.
        if (0x10 <= key.scancode && key.scancode <= 0x19)
            || (0x1E <= key.scancode && key.scancode <= 0x26)
            || (0x2C <= key.scancode && key.scancode <= 0x32)
        {
            if (self.l_shift || self.r_shift) ^ self.caps_lock {
                return Some(key.upper);
            }
        } else {
            if self.l_shift || self.r_shift {
                return Some(key.upper);
            }
        }

        return Some(key.lower);
    }
}

/// Our global keyboard state, protected by a mutex.
static KEYSTATE: Mutex<KeyState> = Mutex::new(KeyState {
    port: unsafe { Port::new(PORT) },
    modifiers: Modifiers::new(),
});

/// Try to read a single input character
pub async fn read() {
    println!("Starting read");

    let mut scancodes = ScancodeStream::new();

    while let Some(scancode) = scancodes.next().await {
        let mut state = KEYSTATE.lock();

        if scancode == 0xE0 {
            // Ignore
            continue;
        }

        // Give our modifiers first crack at this.
        state.modifiers.update(scancode);

        // We don't map any keys > 127.
        if scancode > 127 {
            continue;
        }

        // Look up the ASCII keycode.
        if let Some(key) = KEYS[scancode as usize] {
            // The `as char` converts our ASCII data to Unicode, which is
            // correct as long as we're only using 7-bit ASCII.
            if let Some(transformed_ascii) = state.modifiers.apply_to(key) {
                crate::event::keyboard::dispatch_key_event(&KeyEvent::new(
                    scancode,
                    transformed_ascii,
                    &state.modifiers,
                ));
                continue;
            }
        }
    }
}

/// Called by the keyboard interrupt handler
///
/// Must not block or allocate.
pub(crate) fn add_scancode(scancode: u8) {
    if let Ok(queue) = SCANCODE_QUEUE.try_get() {
        if let Err(_) = queue.push(scancode) {
            println!("WARNING: scancode queue full; dropping keyboard input");
        } else {
            WAKER.wake();
        }
    } else {
        println!("WARNING: scancode queue uninitialized");
    }
}

pub struct ScancodeStream {
    _private: (),
}

impl ScancodeStream {
    pub fn new() -> Self {
        SCANCODE_QUEUE
            .try_init_once(|| ArrayQueue::new(100))
            .expect("ScancodeStream::new should only be called once");

        ScancodeStream { _private: () }
    }
}

impl Stream for ScancodeStream {
    type Item = u8;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<u8>> {
        let queue = SCANCODE_QUEUE
            .try_get()
            .expect("scancode queue not initialized");

        // fast path
        if let Ok(scancode) = queue.pop() {
            return Poll::Ready(Some(scancode));
        }

        WAKER.register(&cx.waker());
        match queue.pop() {
            Ok(scancode) => {
                WAKER.take();
                Poll::Ready(Some(scancode))
            }
            Err(crossbeam_queue::PopError) => Poll::Pending,
        }
    }
}
