"""Exercise ARM UART ingress and task-context delivery with the real line discipline."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def item(source, signature):
    start = source.index(signature)
    at = source.index('{', start)
    depth, end = 1, at + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


class ArmSerialTty(unittest.TestCase):
    def test_uart_bytes_wait_for_discipline_and_keep_order_on_contention(self):
        source = (ROOT / 'kernel/src/ipc/stdin.rs').read_text()
        receiver = item(source, 'pub(crate) fn receive_serial_from_irq(')
        pump = (item(source, 'pub(crate) fn drain_serial_input(')
                if 'pub(crate) fn drain_serial_input(' in source
                else 'pub(crate) fn drain_serial_input() {}')
        buffer = (ROOT / 'kernel/src/ipc/stdin_buffer.rs').read_text()
        # Compile the ARM-only queue observer on the host too.
        buffer = buffer.replace('#[cfg(target_arch = "aarch64")]', '')
        harness = r'''
#![allow(dead_code)]
#![feature(allocator_api)]
extern crate alloc;
use std::sync::{Mutex, MutexGuard};
const STDIN_BUF_SIZE: usize = 4096;
mod buffer { /*BUFFER*/ }
use buffer::StdinBuffer;
struct SerialRing(Mutex<StdinBuffer>);
impl SerialRing {
    const fn new() -> Self { Self(Mutex::new(StdinBuffer::new())) }
    fn try_lock(&self) -> Option<MutexGuard<'_, StdinBuffer>> { self.0.try_lock().ok() }
    fn receive_serial(&self, mut next: impl FnMut() -> Option<u8>) {
        let Some(mut ring) = self.try_lock() else { return };
        while !ring.is_full() {
            let Some(byte) = next() else { return };
            assert!(ring.push_byte(byte));
        }
    }
}
static STDIN_BUFFER: SerialRing = SerialRing::new();
static SERIAL_BUFFER: SerialRing = SerialRing::new();
fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R { f() }
mod tty {
    pub fn read_to_user(_: u64, _: usize, _: impl FnMut(&mut [u8]) -> usize) -> (usize, Result<usize, i32>) { unreachable!() }
    #[path = "TERMIOS"] pub mod termios;
    #[path = "DISCIPLINE"] pub mod line_discipline;
    pub enum InputSource { Serial }
    pub struct Tty;
    pub fn console() -> Option<Tty> { Some(Tty) }
    pub static DISC: std::sync::Mutex<Option<line_discipline::LineDiscipline>> = std::sync::Mutex::new(None);
    impl Tty {
        pub fn input_char_from_nonblock(&self, byte: u8, _: InputSource) -> bool {
            let Ok(mut slot) = DISC.try_lock() else { return false };
            let disc = slot.get_or_insert_with(line_discipline::LineDiscipline::new);
            disc.input_char(byte, &mut |_| {});
            let mut out = [0; 256];
            while let Ok(n) = disc.read(&mut out) {
                if n == 0 { break; }
                let mut cooked = crate::STDIN_BUFFER.0.lock().unwrap();
                for &b in &out[..n] { assert!(cooked.push_byte(b)); }
            }
            true
        }
    }
}
RECEIVER
PUMP
fn read() -> Vec<u8> {
    let mut bytes = [0; STDIN_BUF_SIZE];
    let n = STDIN_BUFFER.0.lock().unwrap().read_bytes(&mut bytes);
    bytes[..n].to_vec()
}
fn feed(bytes: &[u8]) {
    let mut input = bytes.iter().copied();
    receive_serial_from_irq(|| input.next());
}
fn main() {
    feed(b"roox\x7ft");
    assert!(read().is_empty(), "UART bytes bypassed canonical admission");
    let held = tty::DISC.lock().unwrap();
    drain_serial_input();
    assert!(read().is_empty(), "busy discipline leaked raw input");
    drop(held);
    drain_serial_input();
    assert!(read().is_empty(), "partial canonical line became readable");
    feed(b"\r"); drain_serial_input();
    assert_eq!(read(), b"root\n", "canonical erase or ICRNL bypassed");
    feed(b"bad\x15console\r"); drain_serial_input();
    assert_eq!(read(), b"console\n");
    let mut slot = tty::DISC.lock().unwrap();
    let disc = slot.as_mut().unwrap();
    let mut attrs = *disc.termios(); attrs.set_raw(); disc.set_termios(attrs);
    drop(slot);
    feed(b"\r\x7f\x03"); drain_serial_input();
    assert_eq!(read(), b"\r\x7f\x03", "raw input was transformed");
}
'''
        harness = (harness.replace('/*BUFFER*/', buffer).replace('RECEIVER', receiver)
                   .replace('PUMP', pump).replace('TERMIOS', str(ROOT / 'kernel/src/tty/termios.rs'))
                   .replace('DISCIPLINE', str(ROOT / 'kernel/src/tty/line_discipline.rs')))
        with tempfile.TemporaryDirectory(prefix='arm-serial-tty-') as temporary:
            path = Path(temporary)
            (path / 'check.rs').write_text(harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
            subprocess.run([str(path / 'check')], check=True)
        self.assertIn('drain_serial_input()', (ROOT / 'kernel/src/console/mod.rs').read_text())


if __name__ == '__main__':
    unittest.main()
