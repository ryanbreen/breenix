"""Run the production private-context lifecycle against delayed native warm-up."""
from pathlib import Path
import os
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SOURCE = (ROOT / "kernel/src/gpu/window_targets.rs").read_text()


def block(pattern):
    match = re.search(pattern, SOURCE, re.M)
    if match is None:
        raise AssertionError(f"missing production block: {pattern}")
    opening = SOURCE.index("{", match.start())
    depth = 1
    for end in range(opening + 1, len(SOURCE)):
        depth += (SOURCE[end] == "{") - (SOURCE[end] == "}")
        if depth == 0:
            return SOURCE[match.start():end + 1]
    raise AssertionError("unclosed production block")


class WindowContext(unittest.TestCase):
    def test_initialization_and_interrupted_native_lifetimes(self):
        harness = r'''
use std::{cell::{Cell, RefCell}, collections::BTreeMap, sync::{Arc, Mutex, MutexGuard}};
const EIO: i64 = 5;
const EINTR: i64 = 4;
const ENODEV: i64 = 19;
const ENOMEM: i64 = 12;
mod encode { pub const ALL_WARM: u32 = 65535; }
#[derive(PartialEq)] enum CtxState { Live, Quarantined, Reset }
struct Ctx { owner: u64, state: CtxState, dropped: bool, device_alive: bool, warm: u32, warm_oom: bool }
#[derive(Default)]
struct Core { contexts: BTreeMap<u32, Ctx>, handles: BTreeMap<(u64,u64),u32>,
    creations: u32, destroyed: u32, command_pages: u32 }
impl Core {
    fn ctx_of(&self, owner: u64, handle: u64) -> Option<u32> {
        if MODE.with(Cell::get) == 4 { return None; }
        self.handles.get(&(owner,handle)).copied().filter(|id| self.contexts[id].owner == owner)
    }
    fn complete(&mut self) {
        self.command_pages = 0;
        for c in self.contexts.values_mut() {
            if c.state == CtxState::Live { c.warm = encode::ALL_WARM; }
        }
    }
}
thread_local! {
    static CORE: RefCell<Core> = RefCell::default();
    static MODE: Cell<u32> = const { Cell::new(0) };
    static WAITS: Cell<u32> = const { Cell::new(0) };
}
fn with_core<R>(f: impl FnOnce(&mut Core)->R) -> Option<R> {
    Some(CORE.with(|c| f(&mut c.borrow_mut())))
}
struct Cache(Mutex<BTreeMap<u64, Arc<Context>>>);
impl Cache { fn lock(&self)->MutexGuard<'_,BTreeMap<u64,Arc<Context>>> { self.0.lock().unwrap() } }
static CONTEXTS: Cache = Cache(Mutex::new(BTreeMap::new()));
struct Context { owner: u64, handle: u64, id: u32 }
// PRODUCTION_CONTEXT
fn target_context(owner: u64)->Result<Arc<Context>,i64> {
    // PRODUCTION_TARGET_SETUP
    Ok(context)
}
fn create_context(owner: u64)->Result<(u64,u32),i64> {
    with_core(|c| {
        c.creations += 1;
        let id = c.creations;
        c.contexts.insert(id,Ctx { owner, state:CtxState::Live, dropped:false, device_alive:true, warm:0, warm_oom:false });
        c.handles.insert((owner,100 + id as u64),id);
        c.command_pages += 1;
        Ok((100 + id as u64,0))
    }).unwrap()
}
fn destroy_context(owner: u64, handle: u64)->bool {
    with_core(|c| {
        let Some(id) = c.handles.remove(&(owner,handle)) else { return false; };
        c.contexts.get_mut(&id).unwrap().dropped = true;
        c.destroyed += 1;
        // An interrupted waiter cannot free a command still owned by the device.
        true
    }).unwrap()
}
fn wait_for<R>(_: Option<u64>, _: bool, mut f: impl FnMut(&mut Core)->Option<R>)->Result<R,()> {
    if let Some(result) = with_core(|c| f(c)).flatten() { return Ok(result); }
    assert!(CONTEXTS.0.try_lock().is_ok(),"waiting while holding the context cache");
    WAITS.with(|n| n.set(n.get()+1));
    match MODE.with(Cell::get) {
        0 => { with_core(Core::complete); }
        1 => return Err(()),
        5 => { with_core(|c| for x in c.contexts.values_mut() { x.warm_oom = true; }); }
        state => { with_core(|c| for x in c.contexts.values_mut() {
            x.state = if state == 2 { CtxState::Quarantined } else { CtxState::Reset };
        }); }
    }
    Ok(with_core(|c| f(c)).flatten().expect("native completion or failure must resolve the wait"))
}
fn reset(mode: u32) {
    let old = std::mem::take(&mut *CONTEXTS.lock());
    drop(old);
    with_core(|c| *c = Core::default());
    MODE.with(|m|m.set(mode));
    WAITS.with(|n|n.set(0));
    assert_ne!(EIO,ENODEV);
    // The fixture can also compile the old implementation, which has no wait call.
    wait_for(None,true, |_|Some(())).unwrap();
}
#[test]
fn initial_and_cached_contexts_wait_for_native_command_retirement() {
    reset(0);
    let context = target_context(7).unwrap();
    with_core(|c| {
        assert_eq!(c.command_pages,0,"private context escaped while startup DMA remained");
        assert_eq!(c.contexts[&context.id].warm,encode::ALL_WARM);
        // Another caller may find a context published in the cache during its initializer.
        c.contexts.get_mut(&context.id).unwrap().warm = 0;
        c.command_pages = 1;
    });
    let cached = target_context(7).unwrap();
    assert!(Arc::ptr_eq(&context,&cached));
    with_core(|c| {assert_eq!(c.creations,1);assert_eq!(c.command_pages,0);});
    WAITS.with(|n|assert_eq!(n.get(),2));
}
#[test]
fn interruption_quarantine_and_reset_evict_the_failed_initializer() {
    for mode in 1..=5 {
        reset(mode);
        let error = target_context(7).err().expect("unfinished or failed initialization returned a context");
        assert_eq!(error,match mode { 1 => EINTR, 5 => ENOMEM, _ => EIO });
        assert!(CONTEXTS.lock().is_empty(),"failed initializer remained cached");
        with_core(|c| {
            assert_eq!(c.destroyed,1);
            assert!(c.handles.is_empty());
            assert_eq!(c.command_pages,1,"abandoning a wait freed live device memory");
            c.complete();
            assert_eq!(c.command_pages,0);
        });
    }
}
'''
        production = block(r"^impl Context \{") + "\n" + block(r"^impl Drop for Context \{")
        harness = harness.replace("// PRODUCTION_CONTEXT", production)
        setup = re.search(r"fn make\(owner: u64, width: u32, height: u32\).*?\{\s*(let context = [^;]+;)",
                          SOURCE, re.S)
        self.assertIsNotNone(setup, "missing production target setup")
        harness = harness.replace("// PRODUCTION_TARGET_SETUP", setup.group(1))
        with tempfile.TemporaryDirectory(prefix="gpu-window-context-") as directory:
            source = Path(directory) / "context.rs"
            binary = Path(directory) / "context"
            source.write_text(harness)
            subprocess.run([os.environ.get("RUSTC", "rustc"), "--edition=2021", "--test",
                            "-Dwarnings", str(source), "-o", str(binary)], check=True)
            subprocess.run([str(binary), "--test-threads=1"], check=True)


if __name__ == "__main__":
    unittest.main()
