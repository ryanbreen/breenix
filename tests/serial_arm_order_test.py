"""Exercise the ARM record fallback with IRQs masked and an older ring record."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from serial_records_test import function

ROOT = Path(__file__).resolve().parents[1]


class ArmRecordOrder(unittest.TestCase):
    def test_masked_fallback_keeps_queued_records_first(self):
        source = (ROOT / 'kernel/src/serial_line.rs').read_text()
        ring = source[source.index('struct TxRing {'):source.index('/// Held only around copies')]
        names = ['fn terminal_kind(', 'fn emit<', 'fn with_ring<', 'fn queue_terminal(',
                 'fn send_queued(', 'fn send_queued_owned(', 'fn write_queued(']
        functions = '\n'.join(function(source, name) for name in names)
        harness = r'''
#![allow(dead_code)] // Unused hardware branches of the extracted production functions.
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
struct Lock<T>(std::sync::Mutex<T>);
impl<T> Lock<T> {
    const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
    fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
}
const CAPACITY: usize = 1024;
const TX_BYTES: usize = 8192;
const TX_HEADER: usize = 2;
const TX_IDLE: u8 = 0;
const TX_RUNNING: u8 = 1;
static TX: Lock<TxRing> = Lock::new(TxRing::new());
static TX_TID: AtomicU64 = AtomicU64::new(1);
static TX_STATE: AtomicU8 = AtomicU8::new(TX_RUNNING);
static TX_STOPPED: AtomicBool = AtomicBool::new(false);
static WIRE: Lock<Vec<u8>> = Lock::new(Vec::new());
#[derive(Clone, Copy, PartialEq)] enum Writer { Kernel, Tty }
enum Kind { Plain, Record, Line }
mod log { pub enum Level { Info } }
static ORDERED: AtomicUsize = AtomicUsize::new(0);
struct Ownership;
impl Ownership { fn acquire() -> Self { Self } fn acquire_ordered() -> Self { ORDERED.fetch_add(1, Ordering::Relaxed); Self } }
struct Uart;
impl Uart {
    fn mask_interrupts() -> u64 { 0 }
    fn restore_interrupts(_: u64) {}
}
mod uart_owner {
    pub fn write<P, const TEE: bool>(_: super::Writer, bytes: &[u8], _: super::Kind, _: &mut super::Ownership) {
        let _: std::marker::PhantomData<P> = std::marker::PhantomData;
        super::WIRE.lock().extend_from_slice(bytes);
    }
}
mod output_accounting { pub fn count(_: usize, _: bool) {} }
mod console { pub fn log_bytes_at(_: super::log::Level, _: &[u8]) {} }
mod per_cpu {
    pub fn is_initialized() -> bool { true }
    pub fn in_interrupt() -> bool { false }
}
mod task { pub mod scheduler { pub fn wake_thread_any_context(_: u64) {} } }
static THREAD: AtomicBool = AtomicBool::new(false);
fn in_thread_context() -> bool { THREAD.load(Ordering::Relaxed) }
fn write_notices_from_thread() {}
RING
FUNCTIONS
fn scenario(full: bool) {
    WIRE.lock().clear();
    ORDERED.store(0, Ordering::Relaxed);
    let old = [b'a'; 1000];
    let copies = if full { 8 } else { 1 };
    for _ in 0..copies { assert!(queue_terminal(&old)); }
    let new = vec![b'b'; if full { 1000 } else { 2049 }];
    emit::<false, true>(Writer::Tty, &new, log::Level::Info, true);
    assert_eq!(ORDERED.load(Ordering::Relaxed), 1, "normal terminal output can revoke a stalled record");
    let wire = WIRE.lock();
    let mut expected = old.repeat(copies);
    expected.extend_from_slice(&new);
    assert_eq!(*wire, expected, "masked terminal fallback overtook older records");
}
fn main() {
 scenario(false); scenario(true);
 THREAD.store(true, Ordering::Relaxed);
 for attributed in [true,false] {
  ORDERED.store(0, Ordering::Relaxed);WIRE.lock().clear();
  emit::<true,false>(Writer::Kernel,b"exit\n",log::Level::Info,attributed);
  assert_eq!(ORDERED.load(Ordering::Relaxed), usize::from(attributed), "normal logs wait; emergency output may take over");
  assert_eq!(*WIRE.lock(), b"exit\n");
 }
}
'''.replace('RING', ring).replace('FUNCTIONS', functions)
        with tempfile.TemporaryDirectory(prefix='serial-arm-order-') as temporary:
            path = Path(temporary)
            (path / 'check.rs').write_text(harness)
            prefix = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(prefix + ['rustc', '--edition=2021', '-Dwarnings',
                                    str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
            subprocess.run([str(path / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
