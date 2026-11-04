"""Exercise shared claim-loss notification using the production teardown helper."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class ClaimEnd(unittest.TestCase):
    def test_release_and_death_publish_before_waking_readers(self):
        source = (ROOT / 'kernel/src/window/ops.rs').read_text()
        for declaration in ('pub fn release(', 'pub fn group_gone('):
            body = item(source, declaration)
            self.assertIn('end_claim(&mut reg)', body)
            self.assertNotIn('reg.claim = None', body)
        helper = item(source, 'fn end_claim(').replace('fn end_claim(', 'pub fn end_claim(', 1)
        wake = item(source, 'fn claim_ended(').replace('fn claim_ended(', 'pub fn claim_ended(', 1)
        waits = item(source, 'fn all_waits(')
        fixture = r'''
use std::{collections::BTreeMap, sync::{Arc, atomic::{AtomicUsize, Ordering}}};
#[derive(Clone, Copy)] struct Event { kind: u32, epoch: u64 }
mod event_kind { pub const COMPOSITOR_CHANGED: u32 = 13; }
fn kernel_event(kind: u32) -> Event { Event { kind, epoch: 0 } }
struct Ring(Vec<Event>);
impl Ring { fn push(&mut self, event: Event) { self.0.push(event); } }
#[derive(Default)] struct Queue(AtomicUsize);
impl Queue { fn wake_up(&self) { self.0.fetch_add(1, Ordering::SeqCst); } }
#[derive(Default)] struct Waits { events: Queue }
struct Window { ring: Ring, waits: Arc<Waits> }
struct Held;
struct Registry { claim: Option<Held>, epoch: u64, windows: BTreeMap<u64, Window> }
mod ops {
    use super::*;
    // HELPER
    // WAITS
    // WAKE
}
#[test]
fn claim_loss_is_observable_without_another_claim() {
    let mut reg = Registry { claim: Some(Held), epoch: 7, windows: BTreeMap::new() };
    for id in [10, 20] {
        reg.windows.insert(id, Window { ring: Ring(vec![kernel_event(1)]), waits: Arc::default() });
    }
    let woken = ops::end_claim(&mut reg);
    assert!(reg.claim.is_none());
    assert_eq!(reg.epoch, 8);
    assert_eq!(woken.len(), 2);
    for window in reg.windows.values() {
        assert_eq!(window.ring.0.len(), 2);
        let event = window.ring.0.last().unwrap();
        assert_eq!((event.kind, event.epoch), (event_kind::COMPOSITOR_CHANGED, 8));
        assert_eq!(window.waits.events.0.load(Ordering::SeqCst), 0);
    }
    ops::claim_ended(woken);
    for window in reg.windows.values() {
        assert_eq!(window.waits.events.0.load(Ordering::SeqCst), 1);
    }
}
'''.replace('// HELPER', helper).replace('// WAITS', waits).replace('// WAKE', wake)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'claim.rs').write_text(fixture)
            subprocess.run(['rustc', '--edition=2021', '--test', str(path / 'claim.rs'),
                            '-o', str(path / 'claim')], check=True, cwd=ROOT)
            subprocess.run([str(path / 'claim')], check=True)


if __name__ == '__main__':
    unittest.main()
