"""Exercise the real reply lookup with a suspended console-lock holder.

Run directly with python3; rustc compiles only the relevant driver functions.
"""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(source, name):
    start = source.index(f'\npub fn {name}(') + 1
    brace = source.index('{', start)
    depth = 1
    end = brace + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


HARNESS = r'''
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};

struct Mutex<T>(StdMutex<T>);
impl<T> Mutex<T> {
    const fn new(value: T) -> Self { Self(StdMutex::new(value)) }
    fn lock(&self) -> MutexGuard<'_, T> { self.0.lock().unwrap() }
    fn try_lock(&self) -> Option<MutexGuard<'_, T>> { self.0.try_lock().ok() }
}
fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R { f() }
mod tty {
    pub enum InputSource { Other }
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
                for byte in self.0.drain(..) { assert!(admit(byte, super::InputSource::Other)); }
            }
        }
        // DRIVER_FUNCTIONS
        pub fn exercise() {
            *CONSOLE_TTY.lock() = Some(Arc::new(TtyDevice));
            let holder = CONSOLE_TTY.lock();
            // This holder cannot run until masked delivery returns. A blocking
            // lookup here deadlocks, just as with a preempted kernel holder.
            assert!(!crate::arch_without_interrupts(|| push_reply(1, b'R')));
            assert!(INPUT.lock().is_empty());
            drop(holder);
            assert!(crate::arch_without_interrupts(|| push_reply(1, b'R')));
            assert_eq!(*INPUT.lock(), b"R");
        }
    }
}
fn main() { tty::driver::exercise(); }
'''


class ConsoleReplyLockTest(unittest.TestCase):
    def test_contended_console_lookup_returns_for_retry(self):
        source = (ROOT / 'kernel/src/tty/driver.rs').read_text()
        functions = '\n'.join(function(source, name) for name in ('console', 'vt', 'push_reply'))
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary)
            (path / 'reply.rs').write_text(HARNESS.replace('// DRIVER_FUNCTIONS', functions))
            subprocess.run(['rustc', '--edition=2021', '-Dwarnings', str(path / 'reply.rs'),
                            '-o', str(path / 'reply')], check=True)
            process = subprocess.Popen([str(path / 'reply')])
            pid = process.pid
            try:
                self.assertEqual(process.wait(timeout=5), 0)
            except subprocess.TimeoutExpired:
                process.kill()  # Only the child with recorded pid above.
                process.wait()
                self.fail(f'reply lookup deadlocked behind its suspended holder (child {pid})')


if __name__ == '__main__':
    unittest.main()
