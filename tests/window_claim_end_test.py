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
        read = item(source, 'pub fn read_events(')
        fixture = r'''
use std::{collections::BTreeMap, sync::{Arc, atomic::{AtomicUsize, Ordering}}};
#[derive(Clone, Copy)] struct Event { kind: u32, epoch: u64 }
mod event_kind { pub const COMPOSITOR_CHANGED: u32 = 13; }
fn kernel_event(kind: u32) -> Event { Event { kind, epoch: 0 } }
struct Ring(Vec<Event>);
impl Ring { fn push(&mut self, event: Event) { self.0.push(event); } }
impl Ring {
    fn take(&mut self, out: &mut Vec<Event>, capacity: usize) -> u64 {
        let count = self.0.len().min(capacity);
        out.extend(self.0.drain(..count)); 0
    }
    fn untake(&mut self, mut out: Vec<Event>, _: u64) { out.append(&mut self.0); self.0 = out; }
    fn has_event(&self) -> bool { !self.0.is_empty() }
}
#[derive(Default)] struct Queue(AtomicUsize);
impl Queue { fn wake_up(&self) { self.0.fetch_add(1, Ordering::SeqCst); } }
#[derive(Default)] struct Waits { events: Queue }
struct Window { ring: Ring, waits: Arc<Waits> }
#[derive(Clone, Copy)] struct Held { epoch: u64 }
struct Registry { claim: Option<Held>, epoch: u64, windows: BTreeMap<u64, Window> }
struct Caller;
struct ReadEvents { id: u64, ptr: u64, capacity: u32, flags: u32, timeout_ns: u64, lost: u64 }
type WindowEvent = Event;
const READ_EVENTS_BLOCK: u32 = 1;
const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;
mod limits { pub const EVENT_RING: usize = 256; }
static REGISTRY: std::sync::Mutex<Registry> = std::sync::Mutex::new(Registry {
    claim: None, epoch: 0, windows: BTreeMap::new(),
});
struct Lock;
static REGISTRY_KERNEL: Lock = Lock;
impl Lock { fn lock(&self) -> std::sync::MutexGuard<'static, Registry> { REGISTRY.lock().unwrap() } }
impl Registry {
    fn window(&mut self, id: u64, _: &Caller, _: fn(bool,bool,&Caller)->bool) -> Result<&mut Window,i32> {
        self.windows.get_mut(&id).ok_or(2)
    }
}
mod who { use super::Caller; pub fn owner(owner:bool,_:bool,_:&Caller)->bool { owner } }
mod mem {
    pub fn probe_writable(_:u64,_:u64)->Result<(),i32>{Ok(())}
    pub fn copy_out(_:u64,_:&[u8])->Result<(),i32>{Ok(())}
}
mod syscall { pub mod window { pub fn copy_out<T>(_:u64,_:&T)->Result<(),i32>{Ok(())} } }
fn deadline(value:u64)->u64 { value }
fn wait(_: &Queue, _: &u64, pending: impl FnOnce()->bool)->Result<(),i32> {
    assert!(pending());
    let woken = ops::end_claim(&mut REGISTRY.lock().unwrap());
    ops::claim_ended(woken);
    Ok(())
}
mod ops {
    use super::*;
    use super::REGISTRY_KERNEL as REGISTRY;
    // HELPER
    // WAITS
    // WAKE
    // READ
}
#[test]
fn claim_loss_is_observable_without_another_claim() {
    let mut reg = Registry { claim: Some(Held { epoch: 7 }), epoch: 7, windows: BTreeMap::new() };
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
#[test]
fn blocked_old_claim_returns_zero_and_leaves_notification_for_next_read() {
    {
        let mut reg = REGISTRY.lock().unwrap();
        reg.epoch = 7;
        reg.claim = Some(Held { epoch: 7 });
        reg.windows.insert(10, Window { ring: Ring(vec![]), waits: Arc::default() });
    }
    let mut request = ReadEvents { id: 10, ptr: 0, capacity: 4,
        flags: READ_EVENTS_BLOCK, timeout_ns: u64::MAX, lost: 99 };
    assert_eq!(ops::read_events(&Caller, &mut request, 0), Ok(0));
    assert_eq!(request.lost, 0);
    assert_eq!(REGISTRY.lock().unwrap().windows[&10].ring.0.len(), 1);
    request.flags = 0;
    assert_eq!(ops::read_events(&Caller, &mut request, 0), Ok(1));
    assert!(REGISTRY.lock().unwrap().windows[&10].ring.0.is_empty());
}
'''.replace('// HELPER', helper).replace('// WAITS', waits).replace('// WAKE', wake).replace('// READ', read)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'claim.rs').write_text(fixture)
            subprocess.run(['rustc', '--edition=2021', '--test', str(path / 'claim.rs'),
                            '-o', str(path / 'claim')], check=True, cwd=ROOT)
            subprocess.run([str(path / 'claim')], check=True)


if __name__ == '__main__':
    unittest.main()
