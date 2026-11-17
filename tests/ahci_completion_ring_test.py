#!/usr/bin/env python3
"""24.15l: exercise the production recorder and bound its observational reach."""
from pathlib import Path
import hashlib
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
AHCI = ROOT / "kernel/src/drivers/ahci/mod.rs"
RING = AHCI.with_name("command_ring.rs")
COMPLETION = ROOT / "kernel/src/task/completion.rs"


def body(source, name):
    start = source.index("fn " + name + "(")
    opened = source.index("{", start)
    depth, end = 1, opened + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[opened:end]


def without_observations(source):
    # Only standalone, unit-returning observations may be erased. An observation
    # in a predicate, assignment or return must fail instead of hiding a change.
    for name in ("trace_command_event", "trace_retirement", "observe"):
        pattern = re.compile(r"(?m)^[ \t]*" + name + r"\(")
        while match := pattern.search(source):
            depth, end = 1, match.end()
            while depth:
                depth += (source[end] == "(") - (source[end] == ")")
                end += 1
            assert source[end] == ";", "observation must return no decision"
            source = source[:match.start()] + source[end + 1:]
    return source


def normalized(source):
    return re.sub(r"\s+", "", re.sub(r"//[^\n]*", "", source))


class CommandRing(unittest.TestCase):
    def test_record_wrap_and_failure_dump(self):
        # Compile the actual no_std recorder, rather than a second ring model.
        harness = r'''
#[path = "RING_PATH"] mod ring;
use ring::{CommandRing, Event, EventKind};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
static TRACK: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
struct CountAlloc;
unsafe impl GlobalAlloc for CountAlloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if TRACK.load(Ordering::Relaxed) { ALLOCS.fetch_add(1, Ordering::Relaxed); }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { unsafe { System.dealloc(p,l) } }
}
#[global_allocator] static ALLOC: CountAlloc = CountAlloc;
struct Output { bytes: [u8; 16384], len: usize }
impl std::fmt::Write for Output {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self.len + s.len();
        self.bytes[self.len..end].copy_from_slice(s.as_bytes()); self.len = end; Ok(())
    }
}
fn event(n: u32, kind: EventKind) -> Event {
    Event { kind, slot: 0, token: n, ci: 1, port_is: 0x21, tfd: 0x50,
        hba_is: 1, active: 1, done: n - 1, waiter: 0x123456789abcdef0 }
}
#[test]
fn record_wrap_dump() {
    let ring = CommandRing::new();
    let other_port = CommandRing::new();
    let mut output = Output { bytes: [0; 16384], len: 0 };
    TRACK.store(true, Ordering::Relaxed);
    // A different port owns a separate history and dump gate.
    other_port.record(event(99, EventKind::Issue));
    for n in 1..=40 { ring.record(event(n, EventKind::Issue)); }
    // Successful recording has no output path or allocation.
    assert_eq!(output.len, 0);
    ring.dump_once(3, &mut output).unwrap();
    let first_len = output.len;
    ring.dump_once(3, &mut output).unwrap();
    assert_eq!(output.len, first_len);
    TRACK.store(false, Ordering::Relaxed);
    assert_eq!(ALLOCS.load(Ordering::Relaxed), 0);
    let text = std::str::from_utf8(&output.bytes[..output.len]).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 33);
    assert_eq!(lines[0], "[ahci-ring] port=3 newest=40 capacity=32");
    for (i,line) in lines[1..].iter().enumerate() {
        let n = i as u32 + 9;
        assert_eq!(*line, format!("[ahci-ring] seq={n} event=issue slot=0 token={n} CI=0x1 IS=0x21 TFD=0x50 HBA_IS=0x1 active=0x1 done={} waiter=0x123456789abcdef0", n-1));
    }
    // Exercise every discriminating label through the production formatter.
    let labels = [
        (EventKind::Issued, "issued"), (EventKind::IsrEntry, "isr-entry"),
        (EventKind::ReceiptDeferred, "receipt-deferred"),
        (EventKind::ReceiptRecorded, "receipt-recorded"),
        (EventKind::RetireNormal, "retire-normal"), (EventKind::RetireDrain, "retire-drain"),
        (EventKind::RetireSingle, "retire-single-active"),
        (EventKind::WaitPark, "wait-park"), (EventKind::WaitWake, "wait-wake"),
        (EventKind::WaitRecheck, "wait-recheck"), (EventKind::Timeout, "timeout")];
    for (kind,_) in labels { other_port.record(event(100,kind)); }
    let mut other = Output { bytes: [0; 16384], len: 0 };
    other_port.dump_once(7, &mut other).unwrap();
    let text = std::str::from_utf8(&other.bytes[..other.len]).unwrap();
    assert!(text.contains("port=7 newest=12 capacity=32"));
    for (_,label) in labels { assert!(text.contains(&format!("event={label} "))); }
    assert!(!text.contains("token=40 "));
}
'''.replace("RING_PATH", str(RING))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / "ring.rs").write_text(harness)
            subprocess.run(["rustc", "--edition=2021", "-Dwarnings", "--test",
                            str(path / "ring.rs"), "-o", str(path / "ring-test")], check=True)
            subprocess.run([str(path / "ring-test")], check=True, timeout=20)

    def test_ring_is_observational_and_bounded(self):
        source = AHCI.read_text()
        recorder = RING.read_text()
        completion = COMPLETION.read_text()
        for forbidden in (r"\balloc\b", r"\b(?:Vec|Box|String|Mutex|RwLock)\b",
                          r"\.\s*(?:lock|try_lock)\s*\(", r"\bunsafe\b",
                          r"\b(?:print|println|serial_print|serial_println)!", r"format!"):
            self.assertNotRegex(recorder, forbidden)
        record = body(recorder, "record")
        self.assertNotRegex(record, r"\b(?:if|match|loop|while|for|return)\b")
        self.assertIn("fetch_add", record)
        self.assertIn("CAPACITY: usize = 32", recorder)
        self.assertIn("[CommandRing; MAX_PORTS]", source)
        for name in ("trace_command_event", "trace_wait_event", "trace_retirement"):
            observation = body(source, name)
            self.assertNotRegex(observation, r"\.(?:store|swap|fetch_\w+|compare_exchange\w*|lock|try_lock)\(")
            self.assertNotRegex(observation, r"\b(?:return|loop|while|for)\b|serial_print|log::")
        # This guard is specifically for 496(a)'s behavior-preserving diagnostic
        # diff. Hashes are normalized production bodies at d3b12d3e, not output
        # snapshots. Erasing observations must recover those exact statements.
        baseline = {
            "setup_cmd_slot0": "13c6a3ae3786b2106733df9cfb9eac87b642163626136ce5d042adbda9f2f8fb",
            "wait_cmd_slot0": "1251a970e9da8ed223b4c8fa8891bcf0b18c69d3bb6a8bd791e587bcba7f8fca",
            "poll_ports": "d4be1f6ebc974933507ccc2d1315db37d7e89f2ad8c13294b5d4522289e75071",
            "handle_interrupt": "97331ce10ee808dca04520d65814c6df92d76cab87d9b5f352bf16c19fa5878b",
            "detect_completed_slots": "eecf1f43e3970e8809bed432911fcb814aab9b8e9b47a838fc732600aacaa5d8",
        }
        for name, digest in baseline.items():
            actual = normalized(without_observations(body(source, name)))
            actual = actual.replace(
                ".wait_timeout_uninterruptible_observed(cmd_num,TIMEOUT_NS,|stage|trace_wait_event(port,cmd_num,stage))",
                ".wait_timeout_uninterruptible(cmd_num,TIMEOUT_NS)")
            self.assertEqual(hashlib.sha256(actual.encode()).hexdigest(), digest, name)
        observed = normalized(without_observations(body(completion, "wait_timeout_inner_observed")))
        self.assertEqual(hashlib.sha256(observed.encode()).hexdigest(),
                         "ab810fbeb732e417729c911d1333ff47d7f7cb6c6df43418fab77016582534ba")
        self.assertEqual(normalized(body(completion, "wait_timeout_inner")),
                         "{self.wait_timeout_inner_observed(expected_token,timeout_ns,wait_mode,|_|{})}")
        self.assertIn("observe(WaitStage::Park);", completion)
        self.assertIn("observe(WaitStage::Wake);", completion)
        self.assertIn("observe(WaitStage::Recheck);", completion)
        self.assertEqual(source.count(".dump_once("), 1)
        timeout = body(source, "dump_timeout_state_free")
        self.assertIn("EventKind::Timeout", timeout)
        self.assertIn(".dump_once(", timeout)
        self.assertLess(timeout.index("TIMEOUT ("), timeout.index(".dump_once("))
        for name, kind in (("setup_cmd_slot0", "Issue"), ("handle_interrupt", "IsrEntry"),
                           ("handle_interrupt", "ReceiptDeferred"), ("handle_interrupt", "ReceiptRecorded")):
            self.assertIn("EventKind::" + kind, body(source, name))


if __name__ == "__main__":
    unittest.main()
