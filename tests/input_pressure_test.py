"""Exercise production ledger admission and repeat routing off hardware."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class InputPressure(unittest.TestCase):
    def test_pending_custody_cannot_block_console_keys(self):
        source = (ROOT / "kernel/src/input/stream.rs").read_text()
        structs = source[source.index("#[derive(Clone)]"):source.index("static STATE:")]
        prelude = r"""
extern crate alloc;
use alloc::{collections::{BTreeMap, VecDeque}, sync::Arc, vec::Vec};
use input_abi::{axis, disposition, ioctl, key, kind, reason, syn, Capability, Disposition, Event};
const LEDGER_EVENTS: usize = 8192;
const ENOSPC: u32 = 28;
const EAGAIN: u32 = 11;
struct WaitQueueHead;
mod window { pub mod events { pub fn now_ns() -> u64 { 1 } } }
mod event { PURE_EVENT }
mod transport {
use super::*;
"""
        # Strip only module documentation from the production pure key module.
        pure = (ROOT / "kernel/src/input/event.rs").read_text()
        pure = "\n".join(line for line in pure.splitlines() if not line.startswith("//!"))
        program = prelude.replace("PURE_EVENT", pure)
        program += structs
        # Retain production serialization, with no kernel I/O dependencies.
        program += source[source.index("fn batch(records:"):source.index("fn decode(")]
        program += r"""
pub fn check() {
    let mut s = State::new();
    s.devices.insert(1, Device { capabilities: Vec::new(), injector_time: false,
        last_time: 0, modifiers: 0, reserved: 0, held: BTreeMap::new() });
    s.readers.insert(7, Reader { batches: VecDeque::new(), events: 0, dropped: None,
        wake: Arc::new(WaitQueueHead) });
    s.grab = Some(Grab { reader: 7, devices: Vec::new(), holder: 1, epoch: 1 });
    // The compositor read a key for a frozen app; later rows already completed.
    for n in 1..=LEDGER_EVENTS as u64 {
        s.ledger.push_back(Row { event: Event { serial: n, device: 1, ..Event::default() },
            reader: Some(7), outcome: if n == 1 { None } else { Some(Disposition::default()) } });
    }
    s.next_serial = LEDGER_EVENTS as u64 + 1;
    assert!(s.reserve_ledger(4).is_ok(), "unresolved app input blocks console-switch admission");
    assert!(s.ledger.len() + 4 <= LEDGER_EVENTS);
    assert!(s.readers[&7].dropped.is_some(), "custody loss must be reported");
    let records = [29, 56, 59].map(|code| Record { event: Event {
        kind: kind::KEY, code, value: 1, ..Event::default() }, payload: Vec::new() }).to_vec();
    let console = s.publish(1, records);
    assert!(console.iter().any(|e| e.code == 59 && e.reserved));
}
pub fn repeat() {
    let mut s = State::new();
    s.devices.insert(1, Device { capabilities: Vec::new(), injector_time: false,
        last_time: 0, modifiers: 0, reserved: 0, held: BTreeMap::new() });
    s.readers.insert(7, Reader { batches: VecDeque::new(), events: 0, dropped: None,
        wake: Arc::new(WaitQueueHead) });
    s.grab = Some(Grab { reader: 7, devices: Vec::new(), holder: 1, epoch: 1 });
    s.publish(1, vec![Record { event: Event { kind: kind::KEY, code: 14, value: 2,
        ..Event::default() }, payload: Vec::new() }]);
    assert_eq!(s.readers[&7].batches.front().expect("repeat reached grab holder").records[0].event.value, 2);
    assert!(s.ledger[0].outcome.is_none(), "compositor must account for the repeat");
}
}
fn main() { match std::env::args().nth(1).as_deref() {
    Some("repeat") => transport::repeat(), _ => transport::check() } }
"""
        with tempfile.TemporaryDirectory(prefix="input-pressure-") as temp:
            temp = Path(temp)
            rustc = (["taskpolicy", "-b"] if sys.platform == "darwin" else []) + ["rustc", "--edition=2021"]
            env = {**os.environ, "CARGO_BUILD_JOBS": "4"}
            subprocess.run(rustc + ["--crate-name", "input_abi", "--crate-type", "rlib",
                str(ROOT / "libs/input-abi/src/lib.rs"), "--out-dir", str(temp)], check=True, env=env)
            path = temp / "check.rs"
            path.write_text(program)
            result = subprocess.run(rustc + ["-Adead_code", "-Aunused_imports", str(path),
                "--extern", f"input_abi={temp / 'libinput_abi.rlib'}", "-o", str(temp / "check")],
                text=True, capture_output=True, env=env)
            self.assertEqual(result.returncode, 0, result.stderr)
            for case in ["pressure", "repeat"]:
                with self.subTest(case=case):
                    result = subprocess.run([str(temp / "check"), case], text=True, capture_output=True)
                    self.assertEqual(result.returncode, 0, result.stderr)

if __name__ == "__main__":
    unittest.main()
