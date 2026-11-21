"""Host check of the failure-only GUI Apps resident census record."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class GuiMemoryCensus(unittest.TestCase):
    def test_sampling_retries_and_reports_the_terminal_errno(self):
        runner = (ROOT / "libs/libbreenix/src/suite.rs").read_text()
        abi = (ROOT / "libs/gpu-abi/src/lib.rs").read_text()
        verdict_tail = runner[runner.index("let (line, reclaimed) = memory_record::verdict"):]
        verdict_census = item(verdict_tail, 'if self.id == "gui-apps"').replace('self.id', 'id')
        fixture = r'''
#[path = "ERRNO_PATH"]
pub mod errno;
use std::collections::VecDeque;
use std::sync::Mutex;

static RESULTS: Mutex<VecDeque<i64>> = Mutex::new(VecDeque::new());
static EVENTS: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
const YIELD_EVENT: u64 = u64::MAX;
pub const SYS_TEST_CONTROL: u64 = 415;
pub mod gpu {
    pub mod abi {
        pub use crate::{test_control, SYS_TEST_CONTROL};
    }
}
pub mod syscall {
    pub mod raw {
        pub unsafe fn syscall3(number: u64, control: u64, action: u64, len: u64) -> u64 {
            assert_eq!((number, control, len), (crate::SYS_TEST_CONTROL, 5, 0));
            crate::EVENTS.lock().unwrap().push(action);
            crate::RESULTS.lock().unwrap().pop_front().expect("unexpected census call") as u64
        }
    }
}
pub mod process {
    pub fn yield_now() -> Result<(), ()> {
        crate::EVENTS.lock().unwrap().push(crate::YIELD_EVENT);
        Ok(())
    }
}

fn emit(line: &str) {
    LINES.lock().unwrap().push(line.to_owned());
}

fn prepare(statuses: &[i64]) {
    *RESULTS.lock().unwrap() = statuses.iter().copied().collect();
    EVENTS.lock().unwrap().clear();
}

fn assert_events(expected: &[u64]) {
    assert_eq!(*EVENTS.lock().unwrap(), expected);
    assert!(RESULTS.lock().unwrap().is_empty());
}

fn finish(reclaimed: bool, before_census: Option<i64>, after_census: Option<i64>) -> Vec<String> {
    let id = "gui-apps";
    let _ = (before_census, after_census);
    VERDICT_CENSUS
    std::mem::take(&mut *LINES.lock().unwrap())
}

fn main() {
    use errno::Errno::{EAGAIN, EPERM};
    use test_control::resident_census as op;
    let busy = -(EAGAIN as i64);
    let denied = -(EPERM as i64);
    for phase in [op::BEFORE, op::AFTER] {
        prepare(&[busy, busy, 0]);
        assert_eq!(gui_memory_census(phase), 0, "sample lost after transient contention");
        assert_events(&[phase, YIELD_EVENT, phase, YIELD_EVENT, phase]);
    }
    prepare(&[busy, busy, busy, busy]);
    let before = gui_memory_census(op::BEFORE);
    assert_eq!(before, busy);
    assert_events(&[op::BEFORE, YIELD_EVENT, op::BEFORE, YIELD_EVENT,
                    op::BEFORE, YIELD_EVENT, op::BEFORE]);

    prepare(&[busy, denied]);
    let after = gui_memory_census(op::AFTER);
    assert_eq!(after, denied);
    assert_events(&[op::AFTER, YIELD_EVENT, op::AFTER]);

    prepare(&[0]);
    assert_eq!(finish(false, Some(before), Some(after)), [
        "SUITE_MEMORY_PID gui-apps before unavailable=1 errno=11",
        "SUITE_MEMORY_PID gui-apps after unavailable=1 errno=1",
    ]);
    assert_events(&[op::REPORT]);

    prepare(&[0]);
    assert_eq!(finish(false, None, Some(0)), [
        "SUITE_MEMORY_PID gui-apps before unavailable=1 reason=not-sampled",
    ]);
    assert_events(&[op::REPORT]);

    prepare(&[0]);
    assert!(finish(true, Some(before), Some(after)).is_empty());
    assert_events(&[op::DISCARD]);
}
'''.replace("ERRNO_PATH", str(ROOT / "libs/libbreenix/src/errno.rs")).replace(
            "VERDICT_CENSUS", verdict_census
        )
        fixture += item(abi, "pub mod test_control {") + "\n" + item(runner, "fn gui_memory_census(")
        self.run_rust(fixture)

    def test_paired_records_preserve_pid_counts_and_bound(self):
        census = ROOT / "kernel/src/test_control/resident_census/records.rs"
        self.assertTrue(census.is_file(), "the runner has no bounded PID census to report")
        fixture = r'''
#[path = "CENSUS_PATH"]
mod records;
use records::{Census, Pages};

fn lines(census: &Census, phase: &str) -> Vec<String> {
    let mut lines = Vec::new();
    census.report(phase, |line| lines.push(line.to_string()));
    lines
}

fn main() {
    let mut before = Census::new();
    assert!(before.push(7, "/sbin/suite-gui-apps", Pages { user: 150, stack: 16, tables: 8 }));
    assert!(before.push(9, "service\nα", Pages { user: 38, stack: 3, tables: 5 }));

    let mut after = Census::new();
    assert!(after.push(7, "/sbin/suite-gui-apps", Pages { user: 203, stack: 13, tables: 9 }));
    for pid in 10..41 {
        assert!(after.push(pid, "service", Pages { user: 10, stack: 0, tables: 2 }));
    }
    assert!(!after.push(99, "past-bound", Pages { user: 10, stack: 0, tables: 2 }));

    assert_eq!(lines(&before, "before"), [
        "SUITE_MEMORY_PID gui-apps before rows=2 truncated=0 incomplete=0",
        "SUITE_MEMORY_PID gui-apps before pid=7 name=suite-gui-apps user_pages=150 user_stacks=16 page_tables=8",
        "SUITE_MEMORY_PID gui-apps before pid=9 name=service___ user_pages=38 user_stacks=3 page_tables=5",
    ]);
    let after = lines(&after, "after");
    assert_eq!(after.len(), 33);
    assert_eq!(after[0], "SUITE_MEMORY_PID gui-apps after rows=32 truncated=1 incomplete=0");
    assert_eq!(after[1], "SUITE_MEMORY_PID gui-apps after pid=7 name=suite-gui-apps user_pages=203 user_stacks=13 page_tables=9");
    assert!(after[32].contains("pid=40 "));
    assert!(after.iter().all(|line| !line.contains("past-bound")));

    let mut incomplete = Census::new();
    incomplete.incomplete = true;
    assert!(incomplete.push(5, "abcdefghijklmnopqrstuvwxyz0123456789", Pages { user: 0, stack: 0, tables: 1 }));
    assert_eq!(lines(&incomplete, "after"), [
        "SUITE_MEMORY_PID gui-apps after rows=1 truncated=0 incomplete=1",
        "SUITE_MEMORY_PID gui-apps after pid=5 name=abcdefghijklmnopqrstuvwxyz012345 user_pages=0 user_stacks=0 page_tables=1",
    ]);
}
'''.replace("CENSUS_PATH", str(census))
        self.run_rust(fixture)

    @staticmethod
    def run_rust(fixture):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            source = path / "census.rs"
            source.write_text(fixture)
            binary = path / "census"
            subprocess.run(["taskpolicy", "-b", "rustc", "--edition=2021", "-Dwarnings",
                            str(source), "-o", str(binary)], check=True)
            subprocess.run([str(binary)], check=True)


if __name__ == "__main__":
    unittest.main()
