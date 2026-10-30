"""Exercise x86's real formatted serial unit with a thread-local accounting sink."""
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


def item(source, start):
    begin = source.index(start)
    brace = source.index('{', begin)
    depth = 1
    end = brace + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[begin:end]


class X86OutputAccounting(unittest.TestCase):
    def test_formatted_kernel_output_is_charged_to_its_thread(self):
        source = (ROOT / 'kernel/src/serial.rs').read_text()
        unit = item(source, "struct Unit<'a>")
        writer = item(source, "impl fmt::Write for Unit<'_>")
        code = r'''
use std::fmt::{self, Write};
use std::cell::Cell;
thread_local! { static COUNT: Cell<usize> = const { Cell::new(0) }; }
mod output_accounting { pub fn count(n:usize, attributed:bool) { assert!(attributed); super::COUNT.with(|c| c.set(c.get()+n)); } }
#[derive(Clone,Copy,PartialEq)] enum Writer { Kernel, Tty }
struct TxQueue { bytes: Vec<u8> }
fn queue_byte(q:&mut TxQueue, b:u8, _:Writer) { q.bytes.push(b); }
mod serial_writer {
 use super::Writer;
 pub fn lead(_:Writer, _: &[u8])-> &'static [u8] { b"\n" }
 pub fn wrote(_:Writer, _:u8) {}
}
// UNIT
// WRITER
fn main() {
 let _count_fn: fn(usize, bool) = output_accounting::count;
 let worker = std::thread::spawn(|| {
  let mut q=TxQueue {bytes:Vec::new()};
  write!(Unit {queue:&mut q,writer:Writer::Kernel}, "{}\n", "01234567890123456789012345678901").unwrap();
  COUNT.with(|c| assert_eq!(c.get(),33));
  write!(Unit {queue:&mut q,writer:Writer::Tty}, "user").unwrap();
  COUNT.with(|c| assert_eq!(c.get(),33));
 });
 worker.join().unwrap();
 COUNT.with(|c| assert_eq!(c.get(),0));
}
'''.replace('// UNIT', unit).replace('// WRITER', writer)
        with tempfile.TemporaryDirectory() as d:
            path = pathlib.Path(d)
            (path / 'test.rs').write_text(code)
            subprocess.run(['rustc', '--edition=2021', str(path / 'test.rs'), '-o', str(path / 'test')], check=True)
            subprocess.run([str(path / 'test')], check=True)

    def test_direct_kernel_records_are_counted_without_terminal_or_lead_bytes(self):
        source = (ROOT / 'kernel/src/serial.rs').read_text()
        direct = item(source, 'pub fn write_bytes_atomic(')
        self.assertIn('crate::output_accounting::count(bytes.len(), true)', direct)
        self.assertIn('if writer == Writer::Kernel', direct)


if __name__ == '__main__':
    unittest.main()
