"""Exercise the production GR refusal branches and failure snapshot on host IO."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]

class FailureSnapshot(unittest.TestCase):
    def test_refusal_and_accepted_paths(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        issue = item(source, '        fn issue_words(')
        control = issue[issue.index('            if budget < BOUND_US'):issue.index('            Ok(sample)') + len('            Ok(sample)')]
        control = control.replace('self.report.identity.epoch', 'monitor.session.report.identity.epoch').replace('&mut monitor', 'monitor')
        snapshot = item(source, '        fn stream_failure(').replace('read(self.regs,', 'read(&mut self.regs,')
        poll = item(source, 'pub trait Poll')
        observation = '#[derive(Debug)]\n' + item(source, 'pub struct Observation')
        awaiter = item(source, 'fn await_consumed(')
        waiter = item(source, 'pub fn wait(')
        fixture = r'''
#![allow(dead_code)]
use std::sync::Mutex;
static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
#[macro_export]
macro_rules! serial_println { ($($arg:tt)*) => { LINES.lock().unwrap().push(format!($($arg)*)) }; }
const BOUND_US: u64 = 2_000_000;
#[derive(Clone, Copy)]
struct Identity { id: u32, instance: u64, runlist: u32, epoch: u64 }
#[derive(Clone, Copy)]
struct Report { identity: Identity }
#[derive(Clone, Copy)]
struct Session { pbdma: usize, userd: u64, report: Report }
struct Registers;
impl Registers { fn read(&mut self, address: usize) -> u32 { address as u32 } }
fn read(_: &mut Registers, _: u64, address: u64) -> Result<u32, &'static str> { Ok(address as u32) }
struct Monitor { session: Session, regs: Registers, total: u64, now: u64,
    resident_polls: u64, nonresident_polls: u64, consumed: bool }
// POLL
// OBSERVATION
// AWAIT
// WAIT
impl Poll for Monitor {
    fn snapshot(&mut self) -> Result<(u32, u64, bool), &'static str> {
        self.resident_polls += 1;
        Ok((7, 1, self.consumed))
    }
    fn now_us(&mut self) -> u64 { self.now }
    fn relax(&mut self) { self.now += 10; }
}
impl Monitor { // SNAPSHOT
}
fn run(monitor: &mut Monitor, budget: u64) -> Result<Observation, &'static str> {
    let nonce = 7;
    // CONTROL
}
fn monitor(consumed: bool) -> Monitor {
    Monitor { session: Session { pbdma: 0, userd: 0x1000,
        report: Report { identity: Identity { id: 0, instance: 0x2000, runlist: 0, epoch: 1 } } },
        regs: Registers, total: 0, now: 0, resident_polls: 0, nonresident_polls: 0, consumed }
}
#[test]
fn refusal_records_all_fields_once_and_success_is_silent() {
    for budget in [1, BOUND_US] {
        LINES.lock().unwrap().clear();
        let mut m = monitor(false);
        assert!(run(&mut m, budget).is_err());
        let lines = LINES.lock().unwrap();
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert!(line.starts_with("GT710_GR_STREAM_STATE "));
        for field in ["pbdma_40120=0x40120", "pbdma_40014=0x40014", "pbdma_40028=0x40028", "pbdma_3080=0x3080", "userd_48=0x1048", "userd_88=0x1088", "userd_8c=0x108c", "ramfc_48=0x2048", "ch0_800000=0x800000", "ch0_800004=0x800004", "ch1_800000=0x800008", "ch1_800004=0x80000c", "runlist_2284=0x2284", "engine_2640=0x2640", "engine_2644=0x2644", "elapsed_us=2000000", "resident_polls=200001", "nonresident_polls=0"] {
            assert_eq!(line.matches(field).count(), 1, "missing or duplicated {field}: {line}");
        }
    }
    LINES.lock().unwrap().clear();
    let mut m = monitor(true);
    assert!(run(&mut m, 1).unwrap().accepted);
    assert!(LINES.lock().unwrap().is_empty());
}
'''
        for key, value in [('POLL', poll), ('OBSERVATION', observation), ('AWAIT', awaiter),
                           ('WAIT', waiter), ('SNAPSHOT', snapshot), ('CONTROL', control)]:
            fixture = fixture.replace('// ' + key, value)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'fixture.rs').write_text(fixture)
            subprocess.run((['taskpolicy', '-b'] if sys.platform == 'darwin' else []) +
                           ['rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'fixture.rs'), '-o', str(path / 'fixture')], check=True)
            subprocess.run([str(path / 'fixture')], check=True)

if __name__ == '__main__':
    unittest.main()
