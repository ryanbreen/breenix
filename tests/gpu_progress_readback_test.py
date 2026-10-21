"""Exercise production progress-readback enrollment and DMA hold release."""
from pathlib import Path
import os
import re
import subprocess
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[1]
CORE = (ROOT / "kernel/src/gpu/mod.rs").read_text()


def method(name):
    found = re.search(r"(?m)^    fn " + name + r"\(", CORE)
    if found is None:
        raise AssertionError(f"production method {name} is missing")
    opening = CORE.index("{", found.start())
    depth = 1
    for end in range(opening + 1, len(CORE)):
        depth += (CORE[end] == "{") - (CORE[end] == "}")
        if depth == 0:
            return textwrap.dedent(CORE[found.start():end + 1])
    raise AssertionError(f"production method {name} has no closing brace")


class ProgressReadback(unittest.TestCase):
    def test_queueing_and_lifetime(self):
        source = r'''
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
const EIO: i64 = -5;
static REAPER: AtomicU64 = AtomicU64::new(1);
mod platform_config { pub fn is_qemu() -> bool { true } }
mod dev { pub fn is_svga() -> bool { false } }
mod kind { pub const TARGET: u32 = 1; }
mod per_cpu {
    thread_local! { static TID: std::cell::Cell<u64> = const { std::cell::Cell::new(2) }; }
    pub struct Thread { pub id: u64 }
    pub fn current_thread() -> Option<Thread> { Some(Thread { id: TID.with(|t| t.get()) }) }
    pub fn set(id: u64) { TID.with(|t| t.set(id)); }
}
fn notify() {}
enum Tag { Sync(u64) }
struct Read { tag: Tag, ctx: u32, fence: u64, id: u32 }
enum Request { Virgl(Read) }
impl Request {
    fn segment_count(&self) -> usize { 2 }
    fn is_upload(&self) -> bool { false }
    fn fence(&self) -> Option<u64> { let Request::Virgl(r) = self; Some(r.fence) }
}
mod virgl {
    use super::*;
    pub fn transfer_from_host(tag: Tag, ctx: u32, fence: u64, id: u32, _backing: (),
        x: u32, y: u32, w: u32, h: u32, offset: u64, pitch: u32) -> Read {
        assert_eq!((x,y,w,h,offset,pitch), (0,0,1,1,0,256));
        Read { tag, ctx, fence, id }
    }
}
struct Device { room: usize }
impl Device { fn has_room(&self, size: usize) -> bool { self.room >= size } }
struct Writer { ctx: u32, ended: bool }
impl Writer { fn ended(&self) -> bool { self.ended } }
struct Resource {
    kind: u32, writers: u32, accepted: Vec<Writer>, reading: bool, progress_context: Option<u32>,
    invalid: bool, lost: bool, backing: (), pitch: u32, epoch: u64, holds: u32,
}
struct Sub { published: bool, requests: u32, sent_ns: Option<u64>, written: Vec<u32>, ctx: u32 }
struct Ctx { scratch: u32 }
struct SyncReq {
    upload: Option<(u32,u64)>, ctx: u32, sent_ns: Option<u64>, holds: Vec<u32>, reading: Vec<u32>,
    abandoned: bool, failed: bool, why: u32, warm: Option<u64>,
}
struct Core {
    reset_wanted: bool, resetting: bool, stalled: bool, backlog: VecDeque<(u64,Request)>,
    subs: BTreeMap<u64,Sub>, resources: BTreeMap<u32,Resource>, dev: Option<Device>,
    syncs: BTreeMap<u64,SyncReq>, done: BTreeMap<u64,()>, canvas: Vec<(u32,())>,
    epoch: u64, next: u64, next_fence: u64, flushes: u32,
    contexts: BTreeMap<u32,Ctx>,
}
impl Core {
    // PRODUCTION
    fn serial(&mut self) -> u64 { self.next += 1; self.next }
    fn device_fence(&mut self) -> u64 { self.next_fence += 1; self.next_fence }
    fn can_queue(&self, req: &Request) -> bool { self.dev.as_ref().is_some_and(|d| d.has_room(req.segment_count())) }
    fn free_if_unused(&mut self, _id: u32) {}
    fn flush(&mut self) { self.flushes += 1; }
    fn new() -> Self {
        Self { reset_wanted: false, resetting: false, stalled: false, backlog: VecDeque::new(),
            subs: BTreeMap::from([(1,Sub { published: false, requests: 1, sent_ns: Some(0), written: vec![7], ctx: 3 })]),
            resources: BTreeMap::from([(7,Resource { kind: kind::TARGET, writers: 1,
                accepted: vec![Writer { ctx: 3, ended: false }], reading: false, progress_context: None,
                invalid: false, lost: false, backing: (), pitch: 256, epoch: 1, holds: 1 })]),
            dev: Some(Device { room: 2 }), syncs: BTreeMap::new(), done: BTreeMap::new(), canvas: vec![],
            epoch: 1, next: 0, next_fence: 0, flushes: 0,
            contexts: BTreeMap::from([(3,Ctx { scratch: 7 })]) }
    }
}
#[test]
fn drain_stays_queued_and_pinned_until_its_own_completion() {
    let mut c = Core::new();
    c.progress_readback();
    assert_eq!(c.flushes, 0, "the waiter must leave device notification to kgpu");
    assert_eq!(c.backlog.len(), 1);
    let Request::Virgl(read) = &c.backlog[0].1;
    let Tag::Sync(serial) = read.tag;
    assert_eq!((read.ctx, read.id), (3,7));
    assert!(read.fence > 0);
    let s = &c.syncs[&serial];
    assert_eq!(s.ctx, 3);
    assert!(s.sent_ns.is_none() && s.upload.is_none() && s.warm.is_none());
    assert!(s.abandoned && !s.failed && s.why == 0);
    assert_eq!((&s.holds, &s.reading), (&vec![7], &vec![7]));
    assert_eq!(c.resources[&7].holds, 2);
    c.end_reading(7);
    assert!(c.resources[&7].reading);
    assert_eq!(c.resources[&7].progress_context, Some(3));
    c.progress_readback();
    assert_eq!(c.backlog.len(), 1, "only one drain may hold a target");
    c.syncs.remove(&serial);
    c.unhold(7);
    c.end_reading(7);
    assert!(!c.resources[&7].reading);
    assert_eq!(c.resources[&7].progress_context, None);
    assert_eq!(c.resources[&7].holds, 1, "the original submission still owns its hold");
}
#[test]
fn no_drain_behind_backpressure_or_foreign_writers() {
    for variant in 0..5 {
        let mut c = Core::new();
        match variant {
            0 => c.dev.as_mut().unwrap().room = 0,
            1 => c.resources.get_mut(&7).unwrap().reading = true,
            2 => c.resources.get_mut(&7).unwrap().lost = true,
            3 => { let r = c.resources.get_mut(&7).unwrap(); r.writers = 2; r.accepted.push(Writer { ctx: 4, ended: false }); },
            _ => c.subs.get_mut(&1).unwrap().requests = 0,
        }
        c.progress_readback();
        assert!(c.backlog.is_empty() && c.syncs.is_empty());
        assert_eq!(c.resources[&7].holds, 1);
    }
}
#[test]
fn worker_cannot_queue_progress_for_itself() {
    per_cpu::set(REAPER.load(Ordering::Acquire));
    assert_eq!(per_cpu::current_thread().unwrap().id, 1);
    let mut c = Core::new();
    c.progress_readback();
    assert!(c.backlog.is_empty(), "kgpu cannot wait for work only kgpu can send");
}
#[test]
fn ordinary_sync_callers_still_dispatch_their_requests() {
    let mut c = Core::new();
    let r = Request::Virgl(Read { tag: Tag::Sync(20), ctx: 3, fence: 9, id: 7 });
    assert!(c.start_sync(20, r, 3, &[7], &[], None).is_ok());
    assert_eq!(c.flushes, 1);
}
#[test]
fn progress_dma_allows_later_gpu_writers_but_cpu_reads_exclude_them() {
    let mut c = Core::new();
    c.progress_readback();
    let written = [7];
    for id in [3,4] {
        assert!(!({WRITE_BLOCKED}), "a queued progress transfer cannot block a later GPU writer in context {id}");
    }
    c.resources.get_mut(&7).unwrap().progress_context = None;
    let id = 4;
    assert!({WRITE_BLOCKED}, "an ordinary CPU read still excludes GPU context {id}");
}
#[test]
fn drain_reads_the_drawn_target_and_other_contexts_pass_it() {
    let mut c = Core::new();
    c.contexts.get_mut(&3).unwrap().scratch = 9;
    c.resources.insert(9,Resource { kind: kind::TARGET, writers: 0, accepted: vec![],
        reading: false, progress_context: None, invalid: false, lost: false,
        backing: (), pitch: 256, epoch: 1, holds: 1 });
    c.progress_readback();
    let Request::Virgl(read) = &c.backlog[0].1;
    assert_eq!((read.ctx,read.id), (3,7), "the drain must download the target the pending submission draws");
    assert_ne!(read.id, c.contexts[&3].scratch, "a private scratch target is not behind the pending writes");
    assert_eq!((c.resources[&7].holds, c.resources[&9].holds), (2,1));
    assert!(c.resources[&7].reading && !c.resources[&9].reading);
    assert_eq!(c.resources[&7].progress_context, Some(3));
    let written = [7];
    let id = 4;
    assert!(!({WRITE_BLOCKED}), "context {id}'s submission passes a progress transfer it does not own");
}
'''
        methods = "\n".join(method(name) for name in
                            ("start_sync", "queue_sync", "progress_readback", "abandon", "unhold", "end_reading"))
        source = source.replace("// PRODUCTION", methods)
        blocked = re.search(r"if (written\.iter\(\)\.any\([^\n]*r\.reading[^\n]*\)) \{", CORE)
        self.assertIsNotNone(blocked, "production write/read exclusion predicate is missing")
        source = source.replace("{WRITE_BLOCKED}", blocked[1])
        with tempfile.TemporaryDirectory(prefix="gpu-progress-") as directory:
            rust = Path(directory) / "progress.rs"
            binary = Path(directory) / "progress"
            rust.write_text(source)
            subprocess.run([os.environ.get("RUSTC", "rustc"), "--edition=2021", "--test", "-Dwarnings", str(rust), "-o", str(binary)], check=True)
            subprocess.run([str(binary)], check=True)


if __name__ == "__main__":
    unittest.main()
