"""Run the production idle read against a finite kernel timeout.

The diagnostic must identify the path and preserve the failing return value;
this unit measures the failure and does not change timeout handling.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class DispatchDiagnostic(unittest.TestCase):
    def test_idle_timeout_reports_state_without_changing_the_error(self):
        source = (ROOT / 'libs/breengel/src/window.rs').read_text()
        read = item(source, '    fn read(')
        diagnostic = item(source, '    fn read_error(') if '    fn read_error(' in source else ''
        fixture = r'''
#[derive(Debug)] enum Errno { ETIMEDOUT, EIO }
#[derive(Debug)] enum Error { Os(Errno) }
impl std::fmt::Display for Error {
 fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{self:?}") }
}
mod abi {
 pub trait Plain { const ZERO: Self; }
 #[derive(Clone)] pub struct WindowEvent;
 impl Plain for WindowEvent { const ZERO: Self = Self; }
}
mod window {
 use super::*;
 pub fn read_events(id: u64, _: &mut [abi::WindowEvent], block: bool, timeout: u64) -> Result<usize, Error> {
  assert_eq!((id, block, timeout), (42, true, 100_000_000));
  Err(Error::Os(Errno::ETIMEDOUT))
 }
}
mod clock {
 #[derive(Clone, Copy)] pub struct Stamp { pub epoch: u64, pub now_ns: u64 }
 pub fn stamp() -> Stamp { Stamp { epoch: 0, now_ns: 0 } }
 pub fn configured() -> bool { false }
}
mod libbreenix { pub mod time {
 pub struct Time;
 impl Time { pub fn as_nanos(&self) -> u64 { 123_456_789 } }
 pub fn now_monotonic() -> Result<Time, &'static str> { Ok(Time) }
}}
struct Timers;
impl Timers { fn next_deadline(&self) -> u64 { u64::MAX } }
struct Participant;
impl Participant { fn page(&self) -> Self { Self }
 fn wake_generation(&self) -> u32 { 0 }
 fn wait(&self, _: u32, _: u64) {}
}
struct Window { id: u64, clock_participant: Option<Participant>, timers: Timers,
 animating: bool, clock_redraw: bool, ui_stamp: clock::Stamp }
impl Window {
 fn finish_pending(&mut self) -> Result<(), Error> { Ok(()) }
 // READ
 // DIAGNOSTIC
}
fn main() {
 std::hint::black_box(Errno::EIO);
 let mut win = Window { id: 42, clock_participant: None, timers: Timers,
  animating: false, clock_redraw: false, ui_stamp: clock::stamp() };
 assert!(matches!(win.read(true, 100_000_000), Err(Error::Os(Errno::ETIMEDOUT))));
}
'''.replace('// READ', read).replace('// DIAGNOSTIC', diagnostic)
        with tempfile.TemporaryDirectory(prefix='window-dispatch-diagnostic-') as directory:
            path = Path(directory)
            (path / 'check.rs').write_text(fixture)
            compiler = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021']
            # These two helpers are used only after diagnostics are added.
            if not diagnostic:
                fixture = fixture.replace(' pub fn configured() -> bool { false }', '')
                start = fixture.index('mod libbreenix { pub mod time {')
                end = fixture.index('struct Timers;', start)
                fixture = fixture[:start] + fixture[end:]
                (path / 'check.rs').write_text(fixture)
            subprocess.run(compiler + ['-Dwarnings', str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
            result = subprocess.run([str(path / 'check')], capture_output=True, text=True, check=True)
            for field in ['WINDOW_READ_ERROR', 'pid=', 'window=42', 'path=idle-read-events',
                          'participant=false', 'clock_configured=false',
                          'next_timer_ms=18446744073709551615', 'animating=false',
                          'block=true', 'timeout_ns=100000000', 'at_ns=123456789', 'ETIMEDOUT']:
                self.assertIn(field, result.stderr)


if __name__ == '__main__':
    unittest.main()
