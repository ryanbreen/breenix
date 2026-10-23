"""Run the production private-context lifecycle against private clear/copy context creation."""
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
    def test_private_context_cache_and_failed_adoption(self):
        harness = r'''
use std::{cell::{Cell, RefCell}, collections::BTreeMap, sync::{Arc, Mutex, MutexGuard}};
const EIO: i64 = 5;
#[derive(PartialEq)] enum CtxState { Live, Reset }
struct Ctx { owner: u64, state: CtxState, dropped: bool, device_alive: bool }
#[derive(Default)]
struct Core { contexts: BTreeMap<u32, Ctx>, handles: BTreeMap<(u64,u64),u32>,
    creations: u32, destroyed: u32 }
impl Core {
    fn ctx_of(&self, owner: u64, handle: u64) -> Option<u32> {
        if FAIL_ADOPTION.with(Cell::get) { return None; }
        self.handles.get(&(owner,handle)).copied().filter(|id| self.contexts[id].owner == owner)
    }
}
thread_local! {
    static CORE: RefCell<Core> = RefCell::default();
    static FAIL_ADOPTION: Cell<bool> = const { Cell::new(false) };
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
fn create_context_with_catalogue(owner: u64, warm_catalogue: bool)->Result<(u64,u32),i64> {
    assert!(!warm_catalogue, "private context scheduled rendering pipelines");
    with_core(|c| {
        c.creations += 1;
        let id = c.creations;
        c.contexts.insert(id,Ctx { owner, state:CtxState::Live, dropped:false, device_alive:true });
        c.handles.insert((owner,100 + id as u64),id);
        Ok((100 + id as u64,0))
    }).unwrap()
}
fn destroy_context(owner: u64, handle: u64)->bool {
    with_core(|c| {
        let Some(id) = c.handles.remove(&(owner,handle)) else { return false; };
        c.contexts.get_mut(&id).unwrap().dropped = true;
        c.destroyed += 1;
        true
    }).unwrap()
}
fn reset() {
    let old = std::mem::take(&mut *CONTEXTS.lock());
    drop(old);
    with_core(|c| *c = Core::default());
    FAIL_ADOPTION.with(|m|m.set(false));
}
#[test]
fn reuse_live_private_context_and_replace_reset_context() {
    reset();
    let context = target_context(7).unwrap();
    let cached = target_context(7).unwrap();
    assert!(Arc::ptr_eq(&context,&cached));
    with_core(|c| {
        assert_eq!(c.creations,1);
        c.contexts.get_mut(&context.id).unwrap().state = CtxState::Reset;
    });
    let replacement = target_context(7).unwrap();
    assert!(!Arc::ptr_eq(&context,&replacement));
    drop(context);
    drop(cached);
    with_core(|c| { assert_eq!(c.creations,2); assert_eq!(c.destroyed,1); });
}
#[test]
fn failed_adoption_destroys_the_unpublished_handle() {
    reset();
    FAIL_ADOPTION.with(|m|m.set(true));
    assert_eq!(target_context(7).err(),Some(EIO));
    assert!(CONTEXTS.lock().is_empty());
    with_core(|c| { assert_eq!(c.destroyed,1); assert!(c.handles.is_empty()); });
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
