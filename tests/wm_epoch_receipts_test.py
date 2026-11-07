"""W16 production epoch/receipt cost model with host clock and I/O boundaries."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class EpochReceipts(unittest.TestCase):
    def test_epoch_reads_and_completion_validation(self):
        source = (ROOT / 'userspace/programs/src/wm_suite/measure.rs').read_text()
        receipt = item(source, 'pub struct Receipt {')
        parser = item(source, 'pub fn receipts(').replace(
            'std::fs::read_to_string("/proc/breenix/composites")', 'history_text()')
        parser = parser.replace('text.lines()',
            'text.lines().inspect(|_| ROWS.with(|n| n.set(n.get() + 1)))')
        functions = '\n'.join(item(source, marker) for marker in [
            'pub fn completed_commit(', 'pub fn epoch('])
        helpers = '\n'.join(item(source, marker) for marker in [
            'fn checked_receipt(', 'fn completed_in(', 'fn completed_commit_in('] if marker in source)
        fixture = r'''
use std::cell::Cell;
thread_local! { static NOW: Cell<u64> = const { Cell::new(10_000_000_000) }; }
fn now() -> u64 { NOW.with(Cell::get) }
mod protocol { pub const STEP: u32 = 7; }
mod cases { pub fn failure_pipeline() {} }
mod measure {
use super::now;
use std::cell::{Cell, RefCell};
thread_local! {
    static READS: Cell<usize> = const { Cell::new(0) };
    static ROWS: Cell<usize> = const { Cell::new(0) };
    static MODE: Cell<u32> = const { Cell::new(0) };
    static DEADLINES: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}
pub struct Clock { costs: [u64; 3], receipt_reads: u64, receipt_rows: u64 }
impl Clock {
    fn step(&mut self, epoch: u64, ns: u64) -> Result<(), String> {
        assert_eq!((epoch, ns), (77, 77 * 16_000_000));
        Ok(())
    }
}
pub struct Client { id: u64, commit: u64 }
impl Client {
    fn command(&mut self, op: u32, a: u32, b: u32) -> Result<(), String> {
        assert_eq!((op, a, b), (super::protocol::STEP, 77, 1));
        Ok(())
    }
}
fn acknowledged(epoch: u64) -> Result<(), String> { assert_eq!(epoch, 77); Ok(()) }
fn until(ms: u64, what: &str, mut test: impl FnMut() -> Result<bool, String>) -> Result<(), String> {
    assert_eq!(ms, 2000, "missing-client wait bound changed");
    DEADLINES.with(|d| d.borrow_mut().push(ms));
    let end = now() + ms * 1_000_000;
    loop {
        if test()? { return Ok(()); }
        if now() >= end { return Err(format!("{what}: no acknowledgment within {ms} ms")); }
        super::NOW.with(|n| n.set(n.get() + 5_000_000));
    }
}
fn history_text() -> Result<String, std::io::Error> {
    let read = READS.with(|n| { n.set(n.get() + 1); n.get() });
    let mode = MODE.with(Cell::get);
    // A missing client that never publishes remains missing inside its own wait.
    if mode == 4 && read > 1 { return Ok("1 1000000 8 99 1\n".into()); }
    let mut text = String::new();
    for serial in 1..=4096 {
        let window = (serial - 1) % 8 + 1;
        let commit = if (window == 8 && (mode == 4 || mode == 1 && read == 1)) || (mode == 7 && read == 1) || (mode == 8 && (read == 1 || read == 2 && window >= 5)) { 99 } else { 100 };
        let generation = if mode == 2 && window == 8 { 0 } else { 1 };
        let ns = if mode == 3 && window == 8 { now() + 1 } else { 1_000_000 };
        let serial = if mode == 5 && serial == 1 { 0 } else { serial };
        text.push_str(&format!("{serial} {ns} {window} {commit} {generation}\n"));
    }
    Ok(text)
}
#[derive(Clone, Copy)]
RECEIPT
PARSER
HELPERS
FUNCTIONS
fn run(mode: u32) -> Result<u64, String> {
    MODE.with(|n| n.set(mode)); READS.with(|n| n.set(0)); ROWS.with(|n| n.set(0));
    DEADLINES.with(|d| d.borrow_mut().clear()); super::NOW.with(|n| n.set(10_000_000_000));
    let mut clients: Vec<_> = (1..=8).map(|id| Client {
        id, commit: if mode == 6 && id == 8 { 0 } else { 100 },
    }).collect();
    let mut clock = Clock { costs: [0; 3], receipt_reads: 0, receipt_rows: 0 };
    let result = epoch(&mut clock, &mut clients, 77, true);
    assert_eq!(clock.costs.len(), 3);
    assert_eq!(clock.receipt_reads as usize, READS.with(Cell::get));
    if result.is_ok() { assert_eq!(clock.receipt_rows as usize, ROWS.with(Cell::get)); }
    result
}
#[test] fn standalone_completed_commit_keeps_its_wait() {
    run(0).unwrap();
    assert_eq!(completed_commit(&Client { id: 8, commit: 100 }, 100).unwrap().serial, 8);
    assert_eq!(DEADLINES.with(|d| d.borrow().clone()), [2000]);
}
#[test] fn completed_epoch_parses_one_history_for_eight_clients() {
    assert_eq!(run(0).unwrap(), 8);
    let counts = (READS.with(Cell::get), ROWS.with(Cell::get));
    println!("ready epoch history reads={} parsed_rows={}", counts.0, counts.1);
    assert_eq!(counts, (1, 4096), "completed clients repeated the whole history scan");
    assert!(DEADLINES.with(|d| d.borrow().is_empty()));
}
#[test] fn missing_client_rechecks_fresh_history_inside_its_original_wait() {
    assert_eq!(run(1).unwrap(), 8);
    assert_eq!((READS.with(Cell::get), ROWS.with(Cell::get)), (2, 8192));
    assert_eq!(DEADLINES.with(|d| d.borrow().clone()), [2000]);
}
#[test] fn one_refresh_is_reused_for_every_missing_client() {
    assert_eq!(run(7).unwrap(), 8);
    let counts = (READS.with(Cell::get), ROWS.with(Cell::get));
    println!("initially missing epoch history reads={} parsed_rows={}", counts.0, counts.1);
    assert_eq!(counts, (2, 8192), "fresh completed history was discarded between clients");
    assert_eq!(DEADLINES.with(|d| d.borrow().clone()), [2000]);
}
#[test] fn later_completion_cohort_refreshes_only_when_still_missing() {
    assert_eq!(run(8).unwrap(), 8);
    assert_eq!((READS.with(Cell::get), ROWS.with(Cell::get)), (3, 12288));
    assert_eq!(DEADLINES.with(|d| d.borrow().clone()), [2000, 2000]);
}
#[test] fn cached_history_keeps_generation_time_commit_and_row_validation() {
    for (mode, message) in [
        (2, "completion receipt has no completed pixel generation"),
        (3, "completion receipt has no completed pixel generation"),
        (5, "malformed completed-composite receipt"),
        (6, "no input handling commit"),
    ] {
        let error = run(mode).unwrap_err();
        assert!(error.contains(message), "mode={mode}: {error}");
    }
}
#[test] fn stale_commit_cannot_pass_and_missing_wait_still_expires() {
    let error = run(4).unwrap_err();
    assert!(error.contains("no acknowledgment within 2000 ms"), "{error}");
    assert_eq!(now(), 12_000_000_000);
    assert_eq!(DEADLINES.with(|d| d.borrow().clone()), [2000]);
}
}
'''.replace('RECEIPT', receipt).replace('PARSER', parser).replace('HELPERS', helpers).replace('FUNCTIONS', functions)
        with tempfile.TemporaryDirectory(prefix='wm-epoch-receipts-') as directory:
            path = Path(directory)
            (path / 'check.rs').write_text(fixture)
            command = ['rustc', '--edition=2021', '--test', '-Dwarnings',
                       str(path / 'check.rs'), '-o', str(path / 'check')]
            if sys.platform == 'darwin':
                command = ['taskpolicy', '-b', *command]
            subprocess.run(command, check=True)
            subprocess.run([str(path / 'check'), '--test-threads=1', '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
