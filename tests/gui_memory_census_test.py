"""Host check of the failure-only GUI Apps resident census record."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class GuiMemoryCensus(unittest.TestCase):
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
