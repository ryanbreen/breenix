"""Exercise the production outcome lookup against the proc ledger snapshot."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]

class OutcomeQuery(unittest.TestCase):
    def test_lookup_work_and_proc_equivalence(self):
        stream = (ROOT / 'kernel/src/input/stream.rs').read_text()
        row = item(stream, 'fn row_mut(').replace('ledger.get_mut(', 'VISITS.fetch_add(1, Ordering::Relaxed); ledger.get_mut(')
        ledger = item(stream, 'pub fn ledger(').replace('.filter_map(|row|', '.filter_map(|row| { VISITS.fetch_add(1, Ordering::Relaxed);').replace('row.outcome.map(|d| (row.event.serial, d)))', 'row.outcome.map(|d| (row.event.serial, d)) })')
        # Before the new operation exists, exercise the current full-ledger lookup.
        query = item(stream, 'pub fn outcome(') if 'pub fn outcome(' in stream else '''
fn outcome(serial: u64) -> Option<Disposition> {
    let text = ledger();
    let line = text.lines().find(|line| line.split_whitespace().next() == Some(&serial.to_string()))?;
    let fields: Vec<_> = line.split_whitespace().collect();
    Some(Disposition { serial, window: fields[2].parse().unwrap(), disposition: match fields[1] {
        "wm-handled" => 1, "delivered" => 2, "rejected" => 3, "client-rejected" => 4, _ => 5 },
        reason: if fields[3] == "none" { 0 } else { 9 }, ..Disposition::default() })
}'''
        if 'pub fn outcome(' not in stream:
            row = ''
        program = r'''
extern crate alloc;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, atomic::{AtomicUsize, Ordering}};
use input_abi::{Event, Disposition, disposition};
static VISITS: AtomicUsize = AtomicUsize::new(0);
static LOCKS: AtomicUsize = AtomicUsize::new(0);
struct Lock<T>(Mutex<T>, bool);
impl<T> Lock<T> { fn lock(&self) -> std::sync::MutexGuard<'_, T> {
    if self.1 { LOCKS.fetch_add(1, Ordering::Relaxed); } self.0.lock().unwrap() } }
struct Meta { rejected: [u64; 8], rejected_count: u32 }
struct Window { meta: Meta }
struct Registry { windows: BTreeMap<u64, Window> }
mod window { use super::*; pub(super) static REGISTRY: Lock<Registry> = Lock(Mutex::new(Registry { windows: BTreeMap::new() }), true); }
struct Row { event: Event, outcome: Option<Disposition> }
struct State { ledger: VecDeque<Row> }
static STATE: Lock<State> = Lock(Mutex::new(State { ledger: VecDeque::new() }), false);
''' + row + '\n' + ledger + '\n' + query + r'''
fn main() {
 let mut costs = Vec::new();
 for n in [8u64, 8192] {
    { let mut s = STATE.lock(); s.ledger.clear();
      for serial in 100..100+n { s.ledger.push_back(Row { event: Event { serial, ..Event::default() },
        outcome: Some(Disposition { serial, window: 7, disposition: disposition::DELIVERED, ..Disposition::default() }) }); }
      s.ledger[1].outcome = None;
    }
    { let mut r = window::REGISTRY.lock(); r.windows.clear();
      r.windows.insert(7, Window { meta: Meta { rejected: [100+n-1,0,0,0,0,0,0,0], rejected_count: 1 } }); }
    let snapshot = ledger();
    for serial in [100, 100+n-1, 101, 99, 100+n] {
      VISITS.store(0, Ordering::Relaxed); LOCKS.store(0, Ordering::Relaxed);
      let found = outcome(serial);
      let visits = VISITS.load(Ordering::Relaxed); let locks = LOCKS.load(Ordering::Relaxed);
      let line = snapshot.lines().find(|line| line.split_whitespace().next() == Some(&serial.to_string()));
      match (found, line) {
        (Some(d), Some(line)) => { let f: Vec<_> = line.split_whitespace().collect();
          assert_eq!(d.window.to_string(), f[2]); assert_eq!(d.reason, 0);
          assert_eq!(d.disposition, if f[1] == "client-rejected" { 4 } else { 2 });
          println!("rows={n} serial={serial} visits={visits} registry={locks}");
          costs.push((n, visits, locks)); }
        (None, None) => (), _ => panic!("query differs from proc ledger for {serial}")
      }
    }
 }
 for (n, visits, locks) in costs { assert_eq!((visits, locks), (1, 1), "outcome lookup scales at {n} rows"); }
}
'''
        with tempfile.TemporaryDirectory() as directory:
            temp = Path(directory)
            rustc = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021']
            subprocess.run(rustc + ['--crate-name', 'input_abi', '--crate-type', 'rlib', str(ROOT / 'libs/input-abi/src/lib.rs'), '--out-dir', str(temp)], check=True)
            (temp / 'check.rs').write_text(program)
            subprocess.run(rustc + [str(temp / 'check.rs'), '--extern', f'input_abi={temp / "libinput_abi.rlib"}', '-o', str(temp / 'check')], check=True)
            subprocess.run([str(temp / 'check')], check=True)

if __name__ == '__main__': unittest.main()
