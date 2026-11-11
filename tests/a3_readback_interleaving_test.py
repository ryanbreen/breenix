"""Exercise the production SCANOUT_READBACK gate under two poll interleavings."""
from pathlib import Path
import unittest

from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class ReadbackInterleaving(unittest.TestCase):
    def test_completed_frames_remain_observable_during_next_admission(self):
        source = (ROOT / "kernel/src/syscall/window.rs").read_text()
        body = item(source, "fn readback(")
        start = body.index("        if reg.")
        gate = item(body[start:], "if reg.")
        run_rust(self, r'''
mod breengel_abi { pub mod errno { pub const EAGAIN: i32 = 11; } }
#[derive(Clone, Copy)]
struct Held { first_serial: u64 }
struct Record { serial: u64, flags: u32 }
struct Present { serial: u64, busy: bool, records: Vec<Record> }
struct Registry { claim: Option<Held>, present: Present }
fn observe(reg: &Registry) -> Result<u64, i32> {
    @GATE@
    Ok(reg.present.serial)
}
#[test]
fn two_interleavings() {
    let mut reg = Registry {
        claim: Some(Held { first_serial: 10 }),
        present: Present { serial: 10, busy: false, records: vec![] },
    };
    // Startup has not admitted a composite; an idle GPU alone cannot admit observation.
    assert_eq!(observe(&reg), Err(11));
    let mut last_completed = None;
    // A producer finishes between polls, but starts its next frame before each poll.
    // This forces an ordering; there is no host timing or random scheduling here.
    for serial in 11..411 {
        reg.present.busy = false;
        reg.present.serial = serial;
        reg.present.records = vec![Record { serial, flags: 0 }];
        reg.present.busy = true;
        if let Ok(serial) = observe(&reg) { last_completed = Some(serial); }
    }
    assert_eq!(reg.present.serial, 410);
    assert_eq!((reg.present.records[0].serial, reg.present.records[0].flags), (410, 0));
    assert!(reg.present.busy);
    assert_eq!(last_completed, Some(410), "a completed destination must reach the pinned GPU observer despite the next admission");
    // A failed first composite is still pending for a new claim.
    reg.claim = Some(Held { first_serial: 410 });
    reg.present.serial = 411;
    reg.present.records.push(Record { serial: 411, flags: 1 });
    assert_eq!(observe(&reg), Err(11));
    reg.claim = Some(Held { first_serial: 10 });
    // The same completed destination is also observable in the quiet interval.
    reg.present.busy = false;
    assert_eq!(observe(&reg), Ok(411));
}
'''.replace("@GATE@", gate))


if __name__ == "__main__":
    unittest.main()
