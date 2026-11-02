"""Exercise the bounded guest correlation record used to investigate issue 2645."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class QueueTrace(unittest.TestCase):
    def test_identity_wrap_and_freeze_before_reset(self):
        source = ROOT / "kernel/src/gpu/virgl/correlation.rs"
        harness = r'''
#[path = "SOURCE"] mod correlation;
use correlation::{Record, Trace};
fn request(stage: u16, epoch: u64, entry: u64) -> Record {
    Record { at_ns: 2_000_000_001, epoch, entry, fence: entry,
        serial: 1500, detail: 0x8000_0000_0000_1234, context: 40,
        command: 0x207, head: 59, stage, used: 1571, taken: 1571 }
}
#[test]
fn same_request_across_publication_kick_and_observation() {
    let trace = Trace::<8>::new();
    for stage in [1, 2, 3, 4, 5] { trace.record(request(stage, 7, 1572)); }
    let records = trace.snapshot();
    for (index, (ticket, record)) in records.into_iter().flatten().enumerate() {
        assert_eq!(ticket, index as u64 + 1);
        assert_eq!(record, request(index as u16 + 1, 7, 1572));
    }
    assert_eq!(trace.written(), 5);
    assert_eq!(trace.lost(), 0);
}
#[test]
fn reuse_and_counter_wrap_do_not_alias_identity() {
    let trace = Trace::<4>::new();
    let mut wide = request(1, 8, u64::from(u32::MAX) + 1572);
    wide.serial = u64::MAX - 1;
    wide.used = u16::MAX;
    trace.record(request(1, 7, 1572));
    trace.record(wide);
    trace.record(request(5, 8, wide.entry));
    trace.record(request(6, 8, wide.entry));
    trace.record(request(1, 9, wide.entry + 1));
    let records: Vec<_> = trace.snapshot().into_iter().flatten().collect();
    assert_eq!(records.len(), 4);
    assert_eq!(records[0], (2, wide));
    assert_eq!(records[3].1.epoch, 9);
    assert_eq!(records[3].1.head, 59);
    assert_eq!(trace.written(), 5); // The omitted prefix is explicit.
    assert_eq!(trace.lost(), 0); // No writer collision.
}
#[test]
fn first_unexpected_watchdog_preserves_pre_reset_evidence() {
    let trace = Trace::<4>::new();
    trace.record(request(1, 7, 1572));
    trace.record(request(3, 7, 1572));
    trace.record(request(7, 7, 1572));
    assert!(trace.freeze());
    let retained = trace.snapshot();
    for entry in 1573..1600 { trace.record(request(8, 8, entry)); }
    assert_eq!(trace.snapshot(), retained);
    assert_eq!(trace.written(), 3);
    assert!(!trace.freeze());
}
'''.replace("SOURCE", str(source))
        priority = ["taskpolicy", "-b"] if sys.platform == "darwin" else []
        with tempfile.TemporaryDirectory(prefix="gpu-queue-trace-") as directory:
            path = Path(directory)
            (path / "check.rs").write_text(harness)
            subprocess.run(priority + ["rustc", "--edition=2021", "--test", "-Dwarnings",
                                      str(path / "check.rs"), "-o", str(path / "check")], check=True)
            subprocess.run([str(path / "check")], check=True)


if __name__ == "__main__":
    unittest.main()
