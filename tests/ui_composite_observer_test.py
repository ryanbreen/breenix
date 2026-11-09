"""Run the production record reader with a deterministic registry and credentials."""
from pathlib import Path
import subprocess
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
            build = subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', str(path), '-o', str(binary)], capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            run = subprocess.run([str(binary)], capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)


if __name__ == '__main__':
    unittest.main()
