"""Exercise the production SCANOUT_READBACK gate under two poll interleavings."""
from pathlib import Path
import unittest

from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class ReadbackInterleaving(unittest.TestCase):
    def test_completed_frames_can_have_no_successful_observation(self):
        source = (ROOT / "kernel/src/syscall/window.rs").read_text()
        body = item(source, "fn readback(")
        start = body.index("        if reg.present.busy")
        gate = item(body[start:], "if reg.present.busy")
        run_rust(self, r'''
mod breengel_abi { pub mod errno { pub const EAGAIN: i32 = 11; } }
#[derive(Clone, Copy)]
struct Held { first_serial: u64 }
struct Present { serial: u64, busy: bool }
struct Registry { claim: Option<Held>, present: Present }
fn observe(reg: &Registry) -> Result<u64, i32> {
    @GATE@
    Ok(reg.present.serial)
}
#[test]
fn two_interleavings() {
    let mut reg = Registry {
        claim: Some(Held { first_serial: 10 }),
        present: Present { serial: 10, busy: false },
    };
    // Startup has not admitted a composite; an idle GPU alone cannot admit observation.
    assert_eq!(observe(&reg), Err(11));
    let mut last_completed = None;
    // A producer finishes between polls, but starts its next frame before each poll.
    // This forces an ordering; there is no host timing or random scheduling here.
    for serial in 11..411 {
        reg.present.busy = false;
        reg.present.serial = serial;
        reg.present.busy = true;
        if let Ok(serial) = observe(&reg) { last_completed = Some(serial); }
    }
    assert_eq!(reg.present.serial, 410);
    assert_eq!(last_completed, None);
    // The same producer history with a poll in the quiet interval is observable.
    reg.present.busy = false;
    assert_eq!(observe(&reg), Ok(410));
}
'''.replace("@GATE@", gate))


if __name__ == "__main__":
    unittest.main()
