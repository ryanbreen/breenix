"""Feed native console serial records through the production capture and scanner."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class NativeSuiteStatus(unittest.TestCase):
    def test_tty_results_reach_panel_without_entering_kmsg(self):
        fixture = r'''extern crate alloc;
extern crate self as spin;
pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
 pub const fn new(value:T)->Self {Self(std::sync::Mutex::new(value))}
 pub fn try_lock(&self)->Option<std::sync::MutexGuard<'_,T>> {self.0.try_lock().ok()}
}
pub fn arch_interrupts_enabled()->bool {true}
pub mod time {
 pub static NOW: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1234);
 pub fn get_monotonic_time_ns()->(u64,u64) {let ms=NOW.load(core::sync::atomic::Ordering::Relaxed); (ms/1000,(ms%1000)*1_000_000)}
}
pub mod console {#[path="ROOT/kernel/src/console/status.rs"] pub mod status;}
pub mod serial_writer { WRITER }
pub mod log_buffer { LOG_BUFFER }
#[path="ROOT/kernel/src/graphics/boot_producer.rs"] pub mod boot_producer;
#[path="ROOT/kernel/src/graphics/boot_ribbon.rs"] pub mod boot_ribbon;
fn main() {
 use serial_writer::Writer;
 log_buffer::init();
 log_buffer::capture_fmt(format_args!("[metal] kernel idle\n"));
 let tty=b"SUITE console START cases=2\r\nSUITE console CASE text/a PASS ms=1\r\n";
 // TTY record assembly can still hand over a partial line.
 for chunk in tty.chunks(7) {log_buffer::capture_serial_output(Writer::Tty,chunk.iter());}
 log_buffer::capture_serial_output(Writer::Evidence,b"SUITE console CASE text/forged FAIL ms=1 msg=evidence\n".iter());
 log_buffer::capture_serial_output(Writer::Console,b"prompt\n".iter());
 time::NOW.store(62234,core::sync::atomic::Ordering::Relaxed);
 log_buffer::capture_serial_output(Writer::Tty,b"SUITE console CASE text/b FAIL ms=1 msg=bad\r\nSUITE console DONE passed=1 failed=1 skipped=0 total=2\r\n".iter());
 time::NOW.store(999999,core::sync::atomic::Ordering::Relaxed);
 boot_ribbon::drain();
 let suite=boot_ribbon::suite_status().unwrap();
 assert_eq!((suite.passed(),suite.failed(),suite.pending()),(1,1,0));
 assert!(suite.heading(9000).unwrap().contains("1:01"),"DONE did not freeze elapsed");
 assert!(boot_ribbon::reached()[0]);
 let timed=boot_ribbon::serial_timed().unwrap();
 assert!(timed.contains("1234|SUITE console START cases=2"));
 assert!(timed.contains("62234|SUITE console DONE"));
 let serial=boot_ribbon::serial().unwrap();
 assert!(serial.contains("SUITE console DONE"));
 assert!(!serial.contains("forged"));
 assert_eq!(log_buffer::read_all(),"[metal] kernel idle\n");
}
'''.replace('ROOT', str(ROOT))
        fixture = fixture.replace('WRITER', '#[derive(Clone, Copy, PartialEq, Eq)]\n' + item((ROOT / 'kernel/src/serial_writer.rs').read_text(), 'pub enum Writer'))
        log = (ROOT / 'kernel/src/log_buffer.rs').read_text()
        fixture = fixture.replace('LOG_BUFFER', log.replace('#[cfg(target_arch = \"x86_64\")]', ''))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='x86-console-status-') as tmp:
            directory = Path(tmp)
            (directory / 'boot_ribbon.rs').write_text('&["[metal] kernel idle"]')
            (directory / 'check.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                                      str(directory / 'check.rs'), '-o', str(directory / 'check')],
                           check=True, env=dict(os.environ, OUT_DIR=tmp, CARGO_BUILD_JOBS='4'))
            subprocess.run([str(directory / 'check')], check=True)
        writer = item((ROOT / 'kernel/src/serial.rs').read_text(), 'pub fn write_bytes_atomic(')
        self.assertIn('crate::log_buffer::capture_serial_output(writer, lead.iter().chain(bytes))', writer)


if __name__ == '__main__':
    unittest.main()
