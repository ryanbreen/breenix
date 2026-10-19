use std::sync::{Arc, Mutex as StdMutex, MutexGuard};

struct Mutex<T>(StdMutex<T>);
impl<T> Mutex<T> {
    const fn new(value: T) -> Self {
        Self(StdMutex::new(value))
    }
    fn lock(&self) -> MutexGuard<'_, T> {
        self.0.lock().unwrap()
    }
    fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        {
            assert!(
                MASKED.with(|masked| masked.get()),
                "lookup must run under delivery's interrupt mask"
            );
            self.0.try_lock().ok()
        }
    }
}
thread_local! { static MASKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    MASKED.with(|masked| {
        let previous = masked.replace(true);
        let result = f();
        masked.set(previous);
        result
    })
}
mod tty {
    pub enum InputSource {
        Other,
    }
    pub mod driver {
        use super::super::*;
        static CONSOLE_TTY: Mutex<Option<Arc<TtyDevice>>> = Mutex::new(None);
        static OTHER_VTS: std::sync::OnceLock<[Arc<TtyDevice>; 5]> = std::sync::OnceLock::new();
        static INPUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        static PENDING_KEYS: [Mutex<Keys>; 6] = [const { Mutex::new(Keys(Vec::new())) }; 6];
        const KEY_QUEUE_SPINS: usize = 1;
        pub struct TtyDevice;
        impl TtyDevice {
            fn admit_key(&self, byte: u8, _: super::InputSource) -> bool {
                INPUT.lock().push(byte);
                true
            }
        }
        struct Keys(Vec<u8>);
        impl Keys {
            fn push(&mut self, byte: u8, _: super::InputSource) -> bool {
                self.0.push(byte);
                true
            }
            fn drain(&mut self, mut admit: impl FnMut(u8, super::InputSource) -> bool) {
                for byte in self.0.drain(..) {
                    assert!(admit(byte, super::InputSource::Other));
                }
            }
        }
        // DRIVER_FUNCTIONS
        pub fn exercise() {
            *CONSOLE_TTY.lock() = Some(Arc::new(TtyDevice));
            let (held, acquired) = std::sync::mpsc::channel();
            let (release, resume) = std::sync::mpsc::channel();
            let holder = std::thread::spawn(move || {
                let _guard = CONSOLE_TTY.lock();
                held.send(()).unwrap();
                resume.recv().unwrap();
            });
            acquired.recv().unwrap();
            // The holder is a separate suspended thread, released only after
            // delivery returns; a blocking console lookup cannot progress.
            assert!(!crate::arch_without_interrupts(|| push_reply(1, b'R')));
            assert!(INPUT.lock().is_empty());
            release.send(()).unwrap();
            holder.join().unwrap();
            assert!(crate::arch_without_interrupts(|| push_reply(1, b'R')));
            assert_eq!(*INPUT.lock(), b"R");
        }
    }
}
fn main() {
    tty::driver::exercise();
}
