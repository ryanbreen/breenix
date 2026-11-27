#!/usr/bin/env python3
"""Exercise the production procfs read path with one registered entry."""

from pathlib import Path
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
SOURCE = (ROOT / "kernel/src/fs/procfs/mod.rs").read_text()


def function(name):
    start = SOURCE.index(f"pub fn {name}(")
    return SOURCE[start : SOURCE.index("\n}", start) + 2]


FIXTURE = r'''
extern crate alloc;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProcEntryType { TraceReceiveCounters }

impl ProcEntryType {
    fn name(&self) -> &'static str { "rx_counters" }
    fn path(&self) -> &'static str { "/proc/trace/rx_counters" }
}

#[derive(Clone)]
pub struct ProcEntry { entry_type: ProcEntryType }

struct ProcFs { entries: [ProcEntry; 1] }

impl ProcFs {
    fn lock(&self) -> &Self { self }
}

static PROCFS: ProcFs = ProcFs {
    entries: [ProcEntry { entry_type: ProcEntryType::TraceReceiveCounters }],
};

fn lookup_pid_path(_: &str) -> Option<ProcEntry> { None }
fn lookup_trace_teardown_path(_: &str) -> Option<ProcEntry> { None }
fn read_entry(_: ProcEntryType) -> Result<Vec<u8>, i32> { Ok(vec![42]) }

#[test]
fn receive_counters_require_the_registered_path() {
    assert_eq!(read_file("/proc/trace/rx_counters"), Ok(vec![42]));
    assert_eq!(read_file("trace/rx_counters"), Ok(vec![42]));
    assert_eq!(read_file("/proc/rx_counters"), Err(-2));
}
'''


if __name__ == "__main__":
    production = "\n".join(function(name) for name in ("lookup", "lookup_by_path", "read_file"))
    with tempfile.TemporaryDirectory(prefix="breenix-procfs-lookup-") as tmp:
        source = Path(tmp) / "lookup.rs"
        binary = Path(tmp) / "lookup"
        source.write_text(FIXTURE + production)
        subprocess.run(
            ["rustc", "--edition=2021", "-Dwarnings", "--test", str(source), "-o", str(binary)],
            cwd=ROOT,
            check=True,
        )
        sys.exit(subprocess.run([str(binary)]).returncode)
