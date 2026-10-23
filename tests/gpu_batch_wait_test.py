"""Run the production attachment batch against a stalled host-side device model."""
from pathlib import Path
import os
import re
import textwrap
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
CORE = (ROOT / "kernel/src/gpu/mod.rs").read_text()


def function(name):
    match = re.search(r"(?m)^\s*fn\s+" + re.escape(name) + r"\s*\(", CORE)
    if match is None:
        raise AssertionError(f"production function {name} is missing")
    start = CORE.index("fn", match.start())
    opening = CORE.index("{", start)
    depth = 1
    for end in range(opening + 1, len(CORE)):
        depth += (CORE[end] == "{") - (CORE[end] == "}")
        if depth == 0:
            return textwrap.dedent(CORE[start:end + 1]) + "\n"
    raise AssertionError(f"production function {name} has no closing brace")


class BatchWait(unittest.TestCase):
    def test_quarantine_and_production_warm_scheduler(self):
        interval = "\n".join(re.findall(r"const (?:EXAMINE_NS|WARM_GRACE_NS|WARM_QUIET_NS):[^;]+;", CORE))
        harness = r'''
extern crate alloc;
use std::collections::BTreeMap;
use std::cell::RefCell;
const ENOMEM: i64 = -12;
const ENODEV: i64 = -19;
const EIO: i64 = -5;
const EINTR: i64 = -4;
#[derive(Clone, Copy)] enum Tag { Sync(u64), Submit { serial: u64, last: bool }, Background }
struct Request;
struct Done { tag: Tag, answer: u32 }
mod virgl {
    use super::*;
    pub fn ctx_attach(tag: Tag, ctx: u32, id: u32) -> Request {
        let Tag::Sync(serial) = tag else { panic!("attach must be synchronous") };
        assert!(serial > 0 && ctx == 1 && id < 256);
        Request
    }
    pub fn ok(answer: u32) -> bool { answer == 0 }
    pub fn submit(_: Tag, _: u32, _: Option<u64>, _: Words) -> Request { Request }
    pub mod wire { pub mod format { pub const B8G8R8A8_UNORM: u32 = 1; } }
    pub mod encode {
        pub const ALL_WARM: u32 = 0xffff;
        pub struct Enc;
        impl Enc { pub fn new() -> Self { Self } pub fn finish(self) -> Option<Vec<super::Words>> {
            (!crate::OOM.with(std::cell::Cell::get)).then(|| vec![super::Words])
        } }
        pub fn warm_pipeline(_: &mut Enc, _: &mut (), _: u32, _: u32, _: u32, _: u32, _: bool, _: u32) {}
        pub struct TargetRes { pub id: u32, pub format: u32, pub info: super::super::TargetInfo }
        pub struct SourceRes { pub id: u32, pub format: u32, pub width: u32, pub height: u32 }
    }
}
mod svga {
    use super::*;
    pub fn ok(answer: u32) -> bool { answer == 1 }
    pub fn ctx_attach(tag: Tag) -> Request {
        let Tag::Sync(serial) = tag else { panic!("attach must be synchronous") };
        assert!(serial > 0);
        Request
    }
    pub fn submit(_: Tag, _: u32, _: Option<u64>, _: encode::Chunk) -> Option<Request> { Some(Request) }
    pub mod encode {
        use crate::encode::{SourceRes, TargetRes};
        use crate::{format, wire};
        pub struct Chunk;
        pub fn warm_pipeline(_: &mut (), target: &TargetRes, texel: &SourceRes, _: u32) -> Option<Vec<Chunk>> {
            assert_eq!((target.id, target.format, texel.id, texel.format, texel.width, texel.height),
                (2, wire::format::B8G8R8A8_UNORM, 3, wire::format::B8G8R8A8_UNORM, 1, 1));
            let info = &target.info;
            assert_eq!((info.width, info.height, info.invalid, info.format), (1, 1, false, format::B8G8R8A8));
            Some(vec![Chunk])
        }
    }
}
thread_local! { static SVGA: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
mod dev {
    use super::*;
    pub fn is_svga() -> bool { SVGA.with(|s| s.get()) }
    pub fn ok(answer: u32) -> bool { if is_svga() { svga::ok(answer) } else { virgl::ok(answer) } }
    pub fn ctx_attach(tag: Tag, ctx: u32, id: u32) -> Request {
        if is_svga() { svga::ctx_attach(tag) } else { virgl::ctx_attach(tag, ctx, id) }
    }
    pub fn submit_virgl(tag: Tag, ctx: u32, fence: Option<u64>, words: Words) -> Request {
        assert!(!is_svga());
        virgl::submit(tag, ctx, fence, words)
    }
    pub fn submit_svga(tag: Tag, ctx: u32, fence: Option<u64>, chunk: svga::encode::Chunk) -> Option<Request> {
        assert!(is_svga());
        svga::submit(tag, ctx, fence, chunk)
    }
}
mod format { pub const B8G8R8A8: u32 = 2; }
struct TargetInfo { width: u32, height: u32, invalid: bool, format: u32 }
#[derive(Clone, Copy, PartialEq)] enum CtxState { Creating, Live, Quarantined }
use virgl::{encode, wire};
const FLIP_Y: bool = false;
struct Words;
type WordBuf = Words;
impl Words { fn page_count(&self) -> usize { 1 } }
struct WarmStep { ctx: u32, pipeline: u32, left: u32, refused: bool, lost: bool }
struct Ctx { state: CtxState, dropped: bool, owner: u64, device_alive: bool, warm: u32,
    warm_after: u64, warm_catalogue: bool, busy: bool, id: u32, objects: (), dx: (), vb: u32, scratch: u32, scratch_texture: u32 }
fn ctx() -> Ctx { Ctx { state: CtxState::Creating, dropped: false, owner: 1, device_alive: true,
    warm: 0, warm_after: 0, warm_catalogue: true, busy: false, id: 1, objects: (), dx: (), vb: 1, scratch: 2, scratch_texture: 3 } }
thread_local! { static OOM: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
thread_local! { static CLOCK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
fn now_ns() -> u64 { CLOCK.with(|c| c.get()) }
fn notify() {}
fn sweep() {}
struct Sync { sent_ns: Option<u64>, failed: bool, abandoned: bool, warm: Option<u64> }
struct Core {
    contexts: BTreeMap<u32, Ctx>, syncs: BTreeMap<u64, Sync>, done: BTreeMap<u64, Done>,
    dev: Option<()>, reset_wanted: bool, resetting: bool, stalled: bool, quarantined_at: Option<u64>,
    warm_step: Option<WarmStep>, queue_size: usize,
    now: u64, next: u64, traffic_ns: u64, warm_held: Option<u64>, warm_started_ns: Option<u64>,
}
impl Core {
    fn serial(&mut self) -> u64 { self.next += 1; self.next }
    fn start_sync(&mut self, serial: u64, _: Request, ctx: u32, _: &[u32], _: &[u32], warm: Option<u64>) -> Result<(), i64> {
        assert_eq!(ctx, 1);
        self.syncs.insert(serial, Sync { sent_ns: (serial <= 64).then_some(self.now), failed: false, abandoned: warm.is_some(), warm });
        Ok(())
    }
    fn device_fence(&mut self) -> u64 { self.serial() }
    fn settle(&mut self, _: u32) {}
    fn fault(&mut self) { panic!("unexpected fault") }
    fn sync_ended(&mut self, serial: u64, _: Option<Done>) { self.syncs.remove(&serial); }
    fn submit_returned(&mut self, _: u64, _: bool, _: Done) {}
    fn warm_returned(&mut self, _: Option<bool>) { panic!("unexpected warm refusal") }
    // PRODUCTION_METHODS
}
thread_local! { static MODEL: RefCell<Core> = RefCell::new(Core {
    contexts: BTreeMap::from([(1, ctx())]),
    dev: Some(()), reset_wanted: false, resetting: false, stalled: false, quarantined_at: None, warm_step: None, queue_size: 256,
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
        harness = harness.replace("// PRODUCTION_METHODS", function("returned") + function("abandon"))
        chunks = re.search(r"(?m)^enum Chunks \{[^}]*\}\n", CORE)
        if chunks is None:
            raise AssertionError("production enum Chunks is missing")
        harness += interval + chunks.group(0) + function("attach_all") + function("warm_next")
        harness += r'''
#[test]
fn stalled_batched_context_creation_observes_quarantine_without_waiters() {
    stalled_batch(false);
}
#[test]
fn stalled_svga_batched_context_creation_observes_quarantine_without_waiters() {
    stalled_batch(true);
}
fn stalled_batch(svga: bool) {
    SVGA.with(|s| s.set(svga));
    let pairs: Vec<_> = (0..256).map(|id| (1, id)).collect();
    assert_eq!(attach_all(&pairs), vec![Err(EIO); 256]);
    with_core(|c| {
        assert!(c.now - 2_000_000_000 <= 250_000_000);
        assert_eq!(c.syncs.values().filter(|s| s.sent_ns.is_none()).count(), 192);
        assert_eq!(c.syncs.values().filter(|s| !s.abandoned).count(), 0);
        // Production abandon preserves outstanding rows despite caller-visible EIO.
        assert_eq!(c.syncs.len(), 256);
    });
}
#[test]
fn a_failed_warm_encoding_is_retried_without_publishing_completion() {
    with_core(|c| c.contexts.get_mut(&1).unwrap().state = CtxState::Live);
    CLOCK.with(|t|t.set(2_000_000));
    OOM.with(|oom|oom.set(true));
    assert_eq!(warm_next(),None);
    with_core(|c| {
        assert_eq!(c.contexts[&1].warm, 0);
        assert!(c.warm_step.is_none());
        assert!(c.syncs.is_empty());
    });
    OOM.with(|oom|oom.set(false));
    assert_eq!(warm_next(),None);
    with_core(|c| {
        assert!(c.warm_step.is_some());
    });
}
#[test]
fn private_context_does_not_queue_catalogue_dma() {
    with_core(|c| {
        let ctx = c.contexts.get_mut(&1).unwrap();
        ctx.state = CtxState::Live;
        ctx.warm_catalogue = false;
    });
    CLOCK.with(|t|t.set(2_000_000));
    assert_eq!(warm_next(), None);
    with_core(|c| {
        assert!(c.warm_step.is_none());
        assert!(c.syncs.is_empty());
        assert_eq!(c.contexts[&1].warm, 0);
        c.contexts.get_mut(&1).unwrap().warm_catalogue = true;
    });
    assert_eq!(warm_next(), None);
    with_core(|c| assert!(c.warm_step.is_some()));
}
#[test]
fn late_sync_and_submit_answers_space_real_warm_steps() {
    warm_spacing(false);
}
#[test]
fn late_svga_sync_and_submit_answers_space_real_warm_steps() {
    warm_spacing(true);
}
fn warm_spacing(svga: bool) {
    SVGA.with(|s| s.set(svga));
    let answer = if svga { 1 } else { 0 };
    with_core(|c| c.contexts.get_mut(&1).unwrap().state = CtxState::Live);
    CLOCK.with(|t| t.set(2_000_000));
    assert_eq!(warm_next(), None);
    with_core(|c| {
        assert_eq!(c.warm_started_ns, Some(2_000_000));
        let step = c.warm_step.take().unwrap();
        assert_eq!((step.ctx, step.pipeline, step.left, step.refused, step.lost), (1, 0, 1, false, false));
        let serial = *c.syncs.keys().next().unwrap();
        c.returned(Done { tag: Tag::Sync(serial), answer });
        assert_eq!(c.traffic_ns, 0, "warm answer is not competing traffic");
        c.start_sync(99, Request, 1, &[], &[], None).unwrap();
    });
    CLOCK.with(|t| t.set(4_000_000));
    with_core(|c| {
        c.returned(Done { tag: Tag::Sync(99), answer });
        assert!(!c.syncs.contains_key(&99));
        assert_eq!(c.traffic_ns, 4_000_000);
    });
    assert_eq!(warm_next(), Some(22_000_000));
    CLOCK.with(|t| t.set(5_000_000));
    with_core(|c| {
        c.returned(Done { tag: Tag::Submit { serial: 100, last: true }, answer });
        assert_eq!(c.traffic_ns, 5_000_000);
    });
    assert_eq!(warm_next(), Some(22_000_000));
    CLOCK.with(|t| t.set(22_000_000));
    assert_eq!(warm_next(), None);
    with_core(|c| assert_eq!(c.warm_started_ns, Some(22_000_000)));
}
#[test]
fn sixteen_steps_with_sixteen_ms_present_answers() {
    with_core(|c| c.contexts.get_mut(&1).unwrap().state = CtxState::Live);
    let mut now = 2_000_000;
    let first = now;
    let mut present = 16_000_000;
    for pipeline in 0..16 {
        loop {
            CLOCK.with(|t| t.set(now));
            match warm_next() {
                None => break,
                Some(due) => {
                    now = due.min(present);
                    if now == present {
                        CLOCK.with(|t| t.set(now));
                        with_core(|c| c.returned(Done { tag: Tag::Submit { serial: 100, last: true }, answer: 0 }));
                        present += 16_000_000;
                    }
                }
            }
        }
        with_core(|c| {
            let step = c.warm_step.take().expect("step must start");
            assert_eq!(step.pipeline, pipeline);
            c.contexts.get_mut(&1).unwrap().warm |= 1 << pipeline;
        });
        // Model a 3 ms device step and a competing answer during each step.
        now += 1_000_000;
        CLOCK.with(|t| t.set(now));
        with_core(|c| c.returned(Done { tag: Tag::Background, answer: 0 }));
        now += 2_000_000;
    }
    assert_eq!(now - first, 303_000_000);
    println!("16 pipelines with 16 ms presents: {} ns including 3 ms device steps", now - first);
}
'''
        with tempfile.TemporaryDirectory(prefix="gpu-batch-wait-") as folder:
            source, binary = Path(folder) / "batch.rs", Path(folder) / "batch"
            source.write_text(harness)
            subprocess.run([os.environ.get("RUSTC", "rustc"), "--edition=2021", "--test", "-Dwarnings", str(source), "-o", str(binary)], check=True)
            subprocess.run([str(binary)], check=True)


if __name__ == "__main__":
    unittest.main()
