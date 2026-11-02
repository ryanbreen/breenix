"""Run the real unprocessed TTY producer against an interleaved serial sink."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(source, signature):
    start = source.index(signature)
    at = source.index('{', start)
    depth, end = 1, at + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


class SerialRecords(unittest.TestCase):
    def test_fragmented_records_keep_their_bytes_and_bound_unterminated_output(self):
        tty = (ROOT / 'kernel/src/tty/driver.rs').read_text()
        harness = r'''
#![allow(dead_code)] // Kernel entry points and hardware mocks not called by this scenario.
extern crate alloc;
extern crate self as spin;
pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
    pub fn try_lock(&self) -> Option<std::sync::MutexGuard<'_, T>> { self.0.try_lock().ok() }
}
static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
mod time {
    pub fn get_monotonic_time() -> u64 { super::CLOCK.load(std::sync::atomic::Ordering::Relaxed) }
}
mod per_cpu {
    pub fn preempt_disable() {}
    pub fn preempt_enable() {}
    pub fn is_initialized() -> bool { true }
    pub fn current_thread_id_lock_free() -> Option<u64> { Some(super::TID.load(std::sync::atomic::Ordering::Relaxed)) }
}
mod task { pub mod scheduler {
    pub fn current_thread_id() -> Option<u64> { Some(crate::TID.load(std::sync::atomic::Ordering::Relaxed)) }
} }
fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R { f() }
#[path = "WRITER"] mod serial_writer;
static TID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static WIRE: Mutex<Vec<u8>> = Mutex::new(Vec::new());
mod serial {
    pub use super::serial_writer::Writer;
    pub fn sync_log_to_console() {}
    pub fn write_bytes_atomic(writer: Writer, bytes: &[u8]) {
        assert!(bytes.len() <= 4096, "unbounded UART handoff: {}", bytes.len());
        let mut wire = super::WIRE.lock();
        wire.extend_from_slice(super::serial_writer::lead(writer, bytes));
        wire.extend_from_slice(bytes);
        if let Some(&last) = bytes.last() { super::serial_writer::wrote(writer, last); }
    }
}
mod console {
    pub fn write_tty(bytes: &[u8], mut emit: impl FnMut(&[u8])) { emit(bytes); }
    pub fn write_vt(_: usize, _: &[u8]) { unreachable!(); }
}
struct Tty;
impl Tty {
    fn is_console(&self) -> bool { true }
    fn vt(&self) -> usize { 0 }
    COOKED
}
const SERIAL_ATOMIC_OUTPUT_BYTES: usize = 256;
SEGMENTS
PRODUCER
fn diagnostic() { serial::write_bytes_atomic(serial::Writer::Kernel, b"[syscall] exit(0)\n"); }
fn exercise(write_unprocessed: fn(&[u8])) {
    TID.store(1, std::sync::atomic::Ordering::Relaxed);
    // Same fragmentation as #2660: a diagnostic between two write(2)s.
    write_unprocessed(b"SUITE gpu V");
    diagnostic();
    write_unprocessed(b"ALUE performance/coverage-throughput 42\n");
    let wire = WIRE.lock().clone();
    assert!(wire.windows(b"SUITE gpu VALUE performance/coverage-throughput 42\n".len())
        .any(|s| s == b"SUITE gpu VALUE performance/coverage-throughput 42\n"),
        "diagnostic split the userspace record: {:?}", String::from_utf8_lossy(&wire));
    assert!(wire.windows(b"[syscall] exit(0)\n".len()).any(|s| s == b"[syscall] exit(0)\n"));
    WIRE.lock().clear();
    write_unprocessed(b"first writer ");
    TID.store(2, std::sync::atomic::Ordering::Relaxed);
    write_unprocessed(b"second writer\n");
    TID.store(1, std::sync::atomic::Ordering::Relaxed);
    write_unprocessed(b"finished\n");
    let wire = WIRE.lock().clone();
    assert!(wire.windows(b"first writer finished\n".len()).any(|s| s == b"first writer finished\n"),
        "another userspace writer split the line: {:?}", String::from_utf8_lossy(&wire));
    assert!(wire.windows(b"second writer\n".len()).any(|s| s == b"second writer\n"));
    assert_eq!(wire, b"first writer finished\nsecond writer\n", "cross-thread records reordered");
    WIRE.lock().clear();
    write_unprocessed(b"prompt> ");
    CLOCK.fetch_add(1000, std::sync::atomic::Ordering::Relaxed);
    serial_writer::drain_terminal();
    assert!(WIRE.lock().windows(b"prompt> ".len()).any(|s| s == b"prompt> "), "unterminated output never reached the serial port");
    WIRE.lock().clear();
    write_unprocessed(b"discard this fragment");
    serial_writer::discard_terminal();
    serial_writer::drain_terminal_all();
    assert!(WIRE.lock().is_empty(), "flush retained a producer fragment");
    write_unprocessed(b"drain this fragment");
    serial_writer::drain_terminal_all();
    assert!(WIRE.lock().windows(b"drain this fragment".len()).any(|s| s == b"drain this fragment"));
    WIRE.lock().clear();
    let huge = vec![b'z'; 1024 * 1024];
    write_unprocessed(&huge);
    serial_writer::drain_terminal_all();
    assert_eq!(*WIRE.lock(), huge, "large newline-free output lost bytes");
    // Crossing both former chunk boundaries must still be one port handoff.
    WIRE.lock().clear();
    let long = [b'x'; 2048];
    write_unprocessed(&long);
    diagnostic();
    write_unprocessed(b"\n");
    let mut expected = long.to_vec(); expected.push(b'\n');
    assert!(WIRE.lock().windows(expected.len()).any(|s| s == expected), "long record split");
}
fn main() {
    exercise(write_unprocessed);
    exercise(|bytes| Tty.to_sinks(bytes));
}
'''
        harness = (harness.replace('WRITER', str(ROOT / 'kernel/src/serial_writer.rs'))
                   .replace('COOKED', function(tty, 'fn to_sinks('))
                   .replace('SEGMENTS', function(tty, 'fn for_each_line_segment('))
                   .replace('PRODUCER', function(tty, 'pub fn write_unprocessed(')))
        with tempfile.TemporaryDirectory(prefix='serial-records-') as temporary:
            path = Path(temporary)
            (path / 'check.rs').write_text(harness)
            prefix = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(prefix + ['rustc', '--edition=2021', '-Dwarnings',
                            str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
            subprocess.run([str(path / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
