"""Run the production attachment batch against a stalled host-side device model."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
CORE = (ROOT / "kernel/src/gpu/mod.rs").read_text()


def section(start, end):
    return CORE[CORE.index(start):CORE.index(end, CORE.index(start))]


class BatchWait(unittest.TestCase):
    def test_quarantine_ends_unsent_attachments_and_spaces_warm_steps(self):
        attach = section("fn attach_all(", "// ---------------------------------------------------------------------------------------\n// The interrupt")
        interval = section("const EXAMINE_NS:", "static CORE:")
        due = section("        let held = *c.warm_held", "        if now < due")
        harness = r'''
extern crate alloc;
use std::collections::BTreeMap;
use std::cell::RefCell;
const ENOMEM: i64 = -12;
const ENODEV: i64 = -19;
const EIO: i64 = -5;
const EINTR: i64 = -4;
#[derive(Clone, Copy)] enum Tag { Sync(u64) }
struct Request;
struct Done { answer: u32 }
mod virgl {
    use super::*;
    pub fn ctx_attach(Tag::Sync(serial): Tag, ctx: u32, id: u32) -> Request {
        assert!(serial > 0 && ctx == 1 && id < 256);
        Request
    }
    pub fn ok(answer: u32) -> bool { answer == 0 }
}
#[derive(Clone, Copy)] enum CtxState { Creating, Live, Quarantined }
struct Ctx { state: CtxState, dropped: bool }
struct Sync { sent_ns: Option<u64>, failed: bool, abandoned: bool }
struct Core {
    contexts: BTreeMap<u32, Ctx>, syncs: BTreeMap<u64, Sync>, done: BTreeMap<u64, Done>,
    now: u64, next: u64, traffic_ns: u64, warm_held: Option<u64>, warm_started_ns: Option<u64>,
}
impl Core {
    fn serial(&mut self) -> u64 { self.next += 1; self.next }
    fn start_sync(&mut self, serial: u64, _: Request, ctx: u32, _: &[u32], _: &[u32], _: Option<u64>) -> Result<(), i64> {
        assert_eq!(ctx, 1);
        self.syncs.insert(serial, Sync { sent_ns: (serial <= 64).then_some(self.now), failed: false, abandoned: false });
        Ok(())
    }
    fn abandon(&mut self, serial: u64) { self.syncs.get_mut(&serial).unwrap().abandoned = true; }
}
thread_local! { static MODEL: RefCell<Core> = RefCell::new(Core {
    contexts: BTreeMap::from([(1, Ctx { state: CtxState::Creating, dropped: false })]),
    syncs: BTreeMap::new(), done: BTreeMap::new(), now: 0, next: 0,
    traffic_ns: 0, warm_held: None, warm_started_ns: None,
}); }
fn with_core<R>(f: impl FnOnce(&mut Core) -> R) -> Option<R> { MODEL.with(|c| Some(f(&mut c.borrow_mut()))) }
fn ensure_reaper() -> bool { true }
fn wait_for<T>(_: Option<u64>, interruptible: bool, mut f: impl FnMut(&mut Core) -> Option<T>) -> Result<T, ()> {
    assert!(interruptible);
    loop {
        let result = with_core(|c| {
            // A full control queue holds 64 requests; the rest never get sent. The
            // watchdog quarantines at 2 s and cannot reset until 2 s after that.
            if c.now >= 2_000_000_000 {
                c.contexts.get_mut(&1).unwrap().state = CtxState::Quarantined;
                for s in c.syncs.values_mut() {
                    if s.sent_ns.is_some() { s.failed = true; }
                }
            }
            let result = f(c);
            if result.is_none() {
                c.now += EXAMINE_NS;
                assert!(c.now <= 2_250_000_000, "batch waited past quarantine + 250 ms");
            }
            result
        }).flatten();
        if let Some(r) = result { return Ok(r); }
    }
}
'''
        harness += interval + attach
        harness += '\nfn warm_due(c: &mut Core, now: u64) -> u64 {\n' + due + '\n due\n}\n'
        harness += r'''
#[test]
fn stalled_batched_context_creation_observes_quarantine_without_waiters() {
    let pairs: Vec<_> = (0..256).map(|id| (1, id)).collect();
    assert_eq!(attach_all(&pairs), vec![Err(EIO); 256]);
    with_core(|c| {
        assert!(c.now - 2_000_000_000 <= 250_000_000);
        assert_eq!(c.syncs.values().filter(|s| s.sent_ns.is_none()).count(), 192);
        assert_eq!(c.syncs.values().filter(|s| !s.abandoned).count(), 0);
        // Sent requests remain held despite caller-visible EIO.
        assert_eq!(c.syncs.len(), 256);
    });
}
#[test]
fn request_sent_behind_three_ms_step_cannot_allow_another_within_twenty_ms() {
    with_core(|c| {
        c.contexts.get_mut(&1).unwrap().state = CtxState::Live;
        c.warm_started_ns = Some(1_000_000);
        c.traffic_ns = 1_100_000;
        assert_eq!(warm_due(c, 4_000_000), 21_000_000);
        // A late answer restarts quietness; continuous traffic remains bounded.
        c.traffic_ns = 30_000_000;
        assert_eq!(warm_due(c, 30_000_000), 24_000_000);
    });
}
'''
        with tempfile.TemporaryDirectory(prefix="gpu-batch-wait-") as folder:
            source, binary = Path(folder) / "batch.rs", Path(folder) / "batch"
            source.write_text(harness)
            subprocess.run([os.environ.get("RUSTC", "rustc"), "--edition=2021", "--test", "-Dwarnings", str(source), "-o", str(binary)], check=True)
            subprocess.run([str(binary)], check=True)


if __name__ == "__main__":
    unittest.main()
