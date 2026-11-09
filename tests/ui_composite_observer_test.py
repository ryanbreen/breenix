"""Run the production record reader with a deterministic registry and credentials."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class CompositeObserver(unittest.TestCase):
    def test_record_authority_and_live_claim_selection(self):
        reader = item((ROOT / 'kernel/src/window/present.rs').read_text(), 'pub fn record(')
        source = r'''
use std::{collections::VecDeque, sync::{Mutex, MutexGuard}};
const EPERM: i32 = 1; const ENOENT: i32 = 2; const EBUSY: i32 = 16;
#[derive(Clone, Debug, PartialEq)]
struct CompositeEntry { id: u64, pixel_gen: u64, pixels: u64 }
#[derive(Debug, PartialEq)]
struct CompositeRecordHeader { serial: u64, pixels: u64, count: u32, flags: u32 }
impl CompositeRecordHeader { const ZERO: Self = Self { serial: 0, pixels: 0, count: 0, flags: 0 }; }
struct Caller { tgid: u64, euid: u32 }
impl Caller { fn is_root(&self) -> bool { self.euid == 0 } }
#[derive(Clone, Copy)]
struct Claim { holder: u64, first_serial: u64 }
struct Record { serial: u64, pixels: u64, flags: u32, entries: Vec<CompositeEntry> }
struct Present { records: VecDeque<Record> }
struct Registry { claim: Option<Claim>, present: Present }
impl Registry { fn is_compositor(&self, c: &Caller) -> bool { self.claim.is_some_and(|h| h.holder == c.tgid) } }
struct Lock(Mutex<Registry>);
impl Lock { fn lock(&self) -> MutexGuard<'_, Registry> { self.0.lock().unwrap() } }
static REGISTRY: Lock = Lock(Mutex::new(Registry { claim: Some(Claim { holder: 2, first_serial: 10 }),
    present: Present { records: VecDeque::new() } }));
READER
fn main() {
    for (serial, flags) in [(9,0), (11,0), (12,1)] {
        REGISTRY.lock().present.records.push_back(Record { serial, pixels: 0, flags,
            entries: vec![CompositeEntry { id: 7, pixel_gen: 3, pixels: 0 }] });
    }
    let root = Caller { tgid: 1, euid: 0 };
    let compositor = Caller { tgid: 2, euid: 1000 };
    let app = Caller { tgid: 3, euid: 1000 };
    assert_eq!(record(&app, 11), Err(EPERM));
    assert_eq!(record(&app, 999), Err(EPERM));
    assert_eq!(record(&root, 11).unwrap().1[0].pixels, 0);
    assert_eq!(record(&compositor, 11).unwrap().0.serial, 11);
    assert_eq!(record(&root, 0).unwrap().0.serial, 11);
    assert_eq!(record(&compositor, 12).unwrap().0.flags, 1);
    REGISTRY.lock().claim = Some(Claim { holder: 4, first_serial: 12 });
    assert_eq!(record(&root, 0), Err(ENOENT));
    assert_eq!(record(&compositor, 11), Err(EPERM));
    REGISTRY.lock().claim = None;
    assert_eq!(record(&root, 0), Err(ENOENT));
}
'''.replace('READER', reader.replace('pub fn record', 'fn record'))
        with tempfile.TemporaryDirectory(prefix='uitest-record-') as tmp:
            path = Path(tmp) / 'check.rs'
            path.write_text(source)
            binary = Path(tmp) / 'check'
            build = subprocess.run((['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', str(path), '-o', str(binary)], capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            run = subprocess.run([str(binary)], capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)

    def test_wm_ack_uses_only_accepted_source_dispositions(self):
        main = (ROOT / 'userspace/programs/src/bwm.rs').read_text()
        self.assertIn('ack.handled_serial = input::handled_serial();', main,
                      'bwm publishes zero instead of its accepted WM watermark')
        source = (ROOT / 'userspace/programs/src/bwm/input.rs').read_text()
        start = source.index('static HANDLED:')
        atomic = source[start:source.index(';', start) + 1]
        program = r"""
extern crate self as libbreenix;
mod errno { #[derive(Debug)] pub enum Errno { EALREADY } }
mod error { #[derive(Debug)] pub enum Error { Os(super::errno::Errno) }
impl std::fmt::Display for Error { fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result { write!(f,"{self:?}") } } }
mod core_input {
    pub struct Disposition { pub serial:u64, pub window:u64, pub disposition:u32, pub reason:u32, pub derived:u32, pub reserved:u32 }
    pub struct Reader;
    impl Reader { pub fn dispose(&self,d:&Disposition)->Result<(),crate::error::Error> {
        assert_eq!(d.reserved,0); let _ = (d.window,d.disposition,d.reason,d.derived);
        if d.serial == 43 { Err(crate::error::Error::Os(crate::errno::Errno::EALREADY)) } else { Ok(()) }
    } }
}
mod outcome { pub const WM_HANDLED:u32 = 1; }
static READER: std::sync::OnceLock<core_input::Reader> = std::sync::OnceLock::new();
ATOMIC
WATERMARK
DISPOSE
fn main() {
    assert!(READER.set(core_input::Reader).is_ok());
    assert_eq!(handled_serial(),0);
    dispose(42,0,0,outcome::WM_HANDLED,0); assert_eq!(handled_serial(),42);
    dispose(41,0,0,outcome::WM_HANDLED,0); assert_eq!(handled_serial(),42);
    dispose(100,0,7,0,0); assert_eq!(handled_serial(),42);
    dispose(101,0,0,2,0); assert_eq!(handled_serial(),42);
    dispose(102,1,0,outcome::WM_HANDLED,0); assert_eq!(handled_serial(),42);
    dispose(43,0,0,outcome::WM_HANDLED,0); assert_eq!(handled_serial(),42);
    dispose(44,0,0,outcome::WM_HANDLED,0); assert_eq!(handled_serial(),44);
}
""".replace('ATOMIC', atomic).replace('WATERMARK', item(source, 'pub(super) fn handled_serial(').replace('pub(super) ', ''))
        program = program.replace('DISPOSE', item(source, 'fn dispose('))
        with tempfile.TemporaryDirectory(prefix='uitest-wm-watermark-') as tmp:
            path = Path(tmp) / 'check.rs'; path.write_text(program)
            binary = Path(tmp) / 'check'
            command = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '-Dwarnings', str(path), '-o', str(binary)]
            build = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            run = subprocess.run([str(binary)], capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)


if __name__ == '__main__':
    unittest.main()
