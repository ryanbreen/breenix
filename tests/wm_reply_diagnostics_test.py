"""Exercise the production reply reader with deterministic poll and case clocks."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class ReplyDiagnostics(unittest.TestCase):
    def test_reply_and_case_deadlines_have_distinct_failure_evidence(self):
        reader = item((ROOT / 'userspace/programs/src/wm_suite/desktop.rs').read_text(),
                      'fn read_n(')
        fixture = r'''
use std::cell::Cell;
use std::io::Read;
use std::os::fd::{AsRawFd, RawFd};
thread_local! {
    static NOW: Cell<u64> = const { Cell::new(0) };
    static LIMIT: Cell<u64> = const { Cell::new(40_000) };
}
fn now() -> u64 { NOW.with(Cell::get) }
fn case_ms_left() -> u64 { LIMIT.with(Cell::get).saturating_sub(now() / 1_000_000) }
struct File;
impl AsRawFd for File { fn as_raw_fd(&self) -> RawFd { 3 } }
impl Read for File {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        panic!("empty pipe was read without readiness")
    }
}
mod io {
    pub struct PollFd { pub fd: i32, pub events: i16, pub revents: i16 }
    pub fn poll(fds: &mut [PollFd], ms: i32) -> Result<usize, String> {
        assert_eq!((fds[0].fd, fds[0].events, fds[0].revents), (3, 1, 0));
        super::NOW.with(|v| v.set(v.get() + ms as u64 * 1_000_000));
        Ok(0)
    }
}
READER
#[test] fn client_timeout_reports_elapsed_and_remaining_case_budget() {
    let error = read_n(&mut File, &mut [0; 48]).unwrap_err();
    assert!(error.contains("reply deadline"), "{error}");
    assert!(error.contains("waited_ms=2000"), "{error}");
    assert!(error.contains("case_left_ms=38000"), "{error}");
    assert!(!error.contains("case deadline"), "{error}");
}
#[test] fn exhausted_case_is_not_reported_as_a_hung_client() {
    LIMIT.with(|v| v.set(250));
    let error = read_n(&mut File, &mut [0; 48]).unwrap_err();
    assert!(error.contains("case deadline"), "{error}");
    assert!(error.contains("waited_ms=60"), "{error}");
    assert!(error.contains("case_left_ms=190"), "{error}");
    assert!(!error.contains("client exited, hung"), "{error}");
    assert_eq!(now(), 60_000_000);
}
'''.replace('READER', reader)
        with tempfile.TemporaryDirectory(prefix='wm-reply-') as directory:
            path = Path(directory)
            (path / 'test.rs').write_text(fixture)
            command = ['rustc', '--edition=2021', '--test', '-Dwarnings',
                       str(path / 'test.rs'), '-o', str(path / 'test')]
            if sys.platform == 'darwin':
                command = ['taskpolicy', '-b', *command]
            subprocess.run(command, check=True)
            subprocess.run([str(path / 'test')], check=True)

    def test_epoch_failure_names_progress_and_stage_cost(self):
        epoch = item((ROOT / 'userspace/programs/src/wm_suite/measure.rs').read_text(),
                     'pub fn epoch(')
        fixture = r"""
use std::cell::Cell;
thread_local! { static NOW: Cell<u64> = const { Cell::new(0) }; }
fn now() -> u64 { NOW.with(Cell::get) }
mod protocol { pub const STEP: u32 = 7; }
mod measure {
    use super::now;
    pub struct Clock { costs: [u64; 3] }
    impl Clock {
        fn step(&mut self, _: u64, _: u64) -> Result<(), String> { Ok(()) }
    }
    pub struct Client { id: u64, commit: u64 }
    impl Client {
        fn command(&mut self, op: u32, _: u32, _: u32) -> Result<(), String> {
            assert_eq!(op, super::protocol::STEP);
            super::NOW.with(|v| v.set(v.get() + 5_000_000));
            if self.id == 12 { Err("forced case deadline".into()) } else { Ok(()) }
        }
    }
    fn acknowledged(_: u64) -> Result<(), String> { Ok(()) }
    struct Receipt { serial: u64 }
    fn completed_commit(c: &Client, commit: u64) -> Result<Receipt, String> {
        assert_eq!(c.commit, commit);
        Ok(Receipt { serial: 1 })
    }
    EPOCH
    #[test] fn failing_epoch_reports_prior_work_and_current_client() {
        let mut clock = Clock { costs: [25_000_000, 3_000_000, 4_000_000] };
        let mut clients = [Client { id: 11, commit: 3 }, Client { id: 12, commit: 4 }];
        let error = epoch(&mut clock, &mut clients, 61, true).unwrap_err();
        for expected in ["epoch=61", "stage=command", "client=12", "commands_ms=35",
                         "ack_ms=3", "receipts_ms=4", "forced case deadline"] {
            assert!(error.contains(expected), "missing {expected}: {error}");
        }
        assert!(clock.costs[0] >= 25_000_000);
        assert_eq!(now(), 10_000_000);
    }
}
""".replace('EPOCH', epoch)
        with tempfile.TemporaryDirectory(prefix='wm-epoch-') as directory:
            path = Path(directory)
            (path / 'test.rs').write_text(fixture)
            command = ['rustc', '--edition=2021', '--test', '-Dwarnings',
                       str(path / 'test.rs'), '-o', str(path / 'test')]
            if sys.platform == 'darwin':
                command = ['taskpolicy', '-b', *command]
            subprocess.run(command, check=True)
            subprocess.run([str(path / 'test')], check=True)


if __name__ == '__main__':
    unittest.main()
