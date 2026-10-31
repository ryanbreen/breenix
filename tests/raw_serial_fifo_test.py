"""Execute the raw x86 byte writer against a finite, delayed UART FIFO."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class RawSerial(unittest.TestCase):
    def test_ready_poll_preserves_bytes_and_stuck_uart_is_bounded(self):
        source = (ROOT / 'kernel/src/tracing/output.rs').read_text()
        code = item(source, 'pub(crate) fn raw_serial_char(')
        # Only the x86 architecture gate and port I/O are substituted on this host.
        code = code.replace('#[cfg(target_arch = "x86_64")]', '')
        fixture = r'''
struct Device { busy: usize, stuck: bool, reads: usize, writes: usize, lost: usize, accepted: Vec<u8> }
static UART: std::sync::Mutex<Device> = std::sync::Mutex::new(Device {
    busy: 0, stuck: false, reads: 0, writes: 0, lost: 0, accepted: Vec::new() });
mod x86_64 { pub mod instructions { pub mod port {
    pub struct Port<T> { address: u16, kind: core::marker::PhantomData<T> }
    impl Port<u8> {
        pub fn new(address: u16) -> Self { Self { address, kind: core::marker::PhantomData } }
        pub unsafe fn read(&mut self) -> u8 {
            assert_eq!(self.address, 0x3fd); let mut d = crate::UART.lock().unwrap(); d.reads += 1;
            if d.stuck { return 0; }
            d.busy = d.busy.saturating_sub(1); if d.busy == 0 { 0x60 } else { 0 }
        }
        pub unsafe fn write(&mut self, value: u8) {
            assert_eq!(self.address, 0x3f8); let mut d = crate::UART.lock().unwrap(); d.writes += 1;
            if d.stuck || d.busy != 0 { d.lost += 1; }
            else { d.accepted.push(value); d.busy = 8; }
        }
    }
}}}
CODE
#[test] fn finite_fifo_receives_every_capture_byte() {
    *UART.lock().unwrap() = Device { busy: 0, stuck: false, reads: 0, writes: 0, lost: 0, accepted: Vec::new() };
    assert_ne!(unsafe { x86_64::instructions::port::Port::<u8>::new(0x3fd).read() } & 0x20, 0);
    let record = b"[BXCAP:BEGIN edge=PANIC]\r\n[GPU_PANIC_SCREEN shown=1 observed=1]\r\n";
    for &byte in record { raw_serial_char(byte); }
    let (accepted, lost) = { let d = UART.lock().unwrap(); (d.accepted.clone(), d.lost) };
    assert_eq!(accepted, record); assert_eq!(lost, 0);
}
#[test] fn a_stuck_transmitter_is_not_overwritten_or_polled_forever() {
    *UART.lock().unwrap() = Device { busy: 0, stuck: true, reads: 0, writes: 0, lost: 0, accepted: Vec::new() };
    raw_serial_char(b'X');
    let (writes, reads) = { let d = UART.lock().unwrap(); (d.writes, d.reads) };
    assert_eq!(writes, 0); assert!(reads > 0 && reads <= 1024);
}
'''.replace('CODE', code)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            (p / 'serial.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                str(p / 'serial.rs'), '-o', str(p / 'serial')], check=True)
            subprocess.run([str(p / 'serial'), '--test-threads=1', '--nocapture'], check=True)

    def test_capture_writer_reports_a_transmit_refusal(self):
        source = (ROOT / 'kernel/src/capture/record.rs').read_text()
        code = item(source, '    fn put(')
        code = code.replace('#[cfg(target_arch = "x86_64")]', '')
        code = code.replace('#[cfg(target_arch = "aarch64")]', '#[cfg(any())]')
        fixture = r'''
static READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn raw_serial_char(_: u8) -> bool { READY.load(std::sync::atomic::Ordering::Relaxed) }
struct Writer { budgeted: bool, remaining: u32, truncated: bool, bytes: u32 }
impl Writer { CODE }
fn main() {
    let mut w = Writer { budgeted: true, remaining: 10, truncated: false, bytes: 0 };
    assert!(!w.put(b'X'), "an untransmitted byte must not be counted as written");
    assert!(w.truncated); assert_eq!(w.remaining, 0); assert_eq!(w.bytes, 0);
    READY.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(!w.put(b'Y'), "section content stays truncated after transport loss");
    w.budgeted = false; assert!(w.put(b'E'), "the unbudgeted END may report truncation");
    assert_eq!(w.bytes, 1);
}
'''.replace('CODE', code)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            (p / 'writer.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                str(p / 'writer.rs'), '-o', str(p / 'writer')], check=True)
            subprocess.run([str(p / 'writer')], check=True)


if __name__ == '__main__':
    unittest.main()
