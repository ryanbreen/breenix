#!/usr/bin/env python3
"""Run production group-child and credential logic against explicit process-row shims."""
from pathlib import Path
import os
import subprocess
import tempfile
ROOT = Path(__file__).resolve().parents[1]
def function(source, name):
    start = source.index("fn " + name + "(")
    brace = source.index("{", start)
    depth = 1
    pos = brace + 1
    while depth:
        depth += (source[pos] == "{") - (source[pos] == "}")
        pos += 1
    return source[start:pos]
manager = (ROOT / "kernel/src/process/manager.rs").read_text()
handlers = (ROOT / "kernel/src/syscall/handlers.rs").read_text()
methods = "\n".join(function(manager, n) for n in ["wait_children", "thread_group_peers", "reparent_children_to_init"])
change = function(handlers, "change_credentials")
harness = r'''
extern crate alloc;
use alloc::vec::Vec;
use std::collections::BTreeMap;
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ProcessId(u64);
impl ProcessId { pub fn as_u64(self) -> u64 { self.0 } }
pub mod fs { pub mod ext2 {
    pub struct Ext2Inode;
    impl Ext2Inode {
        pub fn permissions(&self) -> u16 { 0 }
        pub fn uid(&self) -> u32 { 0 }
        pub fn gid(&self) -> u32 { 0 }
    }
} }
#[path = "CREDENTIALS_PATH"]
pub mod credentials;
pub struct Row {
    pub thread_group_id: Option<u64>,
    pub children: Vec<ProcessId>,
    pub parent: Option<ProcessId>,
    pub exit_started: bool,
    pub terminated: bool,
    pub cred: credentials::ProcessCredentials,
    pub published_identity: std::cell::Cell<(u32, u32, u32)>,
}
impl Row {
    fn is_terminated(&self) -> bool { self.terminated }
    // This harness has no signal mirror; retain the published credential identity.
    fn publish_identity(&self) {
        self.published_identity.set((self.cred.uid, self.cred.euid, self.cred.suid));
    }
}
pub struct Rows(BTreeMap<ProcessId, Row>);
impl Rows {
    fn live_row(&self, pid: &ProcessId) -> Option<&Row> { self.0.get(pid) }
    fn live_row_mut(&mut self, pid: &ProcessId) -> Option<&mut Row> { self.0.get_mut(pid) }
    fn iter(&self) -> std::collections::btree_map::Iter<'_, ProcessId, Row> { self.0.iter() }
}
pub struct Manager { processes: Rows, designated_init: Option<ProcessId> }
impl Manager {
    fn find_process_by_thread(&self, _: u64) -> Option<(ProcessId, &Row)> {
        self.processes.live_row(&ProcessId(10)).map(|r| (ProcessId(10), r))
    }
    fn get_process_mut(&mut self, pid: ProcessId) -> Option<&mut Row> { self.processes.live_row_mut(&pid) }
METHODS
}
fn row(group: u64, children: &[u64]) -> Row {
    Row { thread_group_id: Some(group), children: children.iter().map(|&p| ProcessId(p)).collect(),
          parent: Some(ProcessId(1)), exit_started: false, terminated: false, cred: credentials::ProcessCredentials::root(),
          published_identity: std::cell::Cell::new((0, 0, 0)) }
}
fn manager() -> Manager {
    let mut rows = BTreeMap::new();
    rows.insert(ProcessId(1), row(1, &[]));
    rows.insert(ProcessId(10), row(10, &[11, 20]));
    rows.insert(ProcessId(11), row(10, &[21]));
    rows.insert(ProcessId(20), row(20, &[]));
    rows.insert(ProcessId(21), row(21, &[]));
    rows.insert(ProcessId(30), row(30, &[31]));
    Manager { processes: Rows(rows), designated_init: Some(ProcessId(1)) }
}
pub mod tracing { pub mod providers { pub mod teardown {
    pub fn record_init_reparent(_: usize, _: bool) {}
} } }
pub fn arch_without_interrupts<T>(f: impl FnOnce() -> T) -> T { f() }
pub mod task { pub mod scheduler { pub fn current_thread_id() -> Option<u64> { Some(7) } } }
pub mod process {
    pub use crate::credentials;
    pub fn manager() -> std::sync::MutexGuard<'static, Option<crate::Manager>> {
        static STATE: std::sync::Mutex<Option<crate::Manager>> = std::sync::Mutex::new(None);
        STATE.lock().unwrap()
    }
}
pub mod syscall {
    pub mod errno { pub const EINVAL: i32 = 22; pub const EPERM: i32 = 1; pub const ESRCH: i32 = 3; pub const ECHILD: i32 = 10; pub const ENOMEM: i32 = 12; }
    #[derive(Debug, PartialEq)]
    pub enum SyscallResult { Ok(u64), Err(u64) }
    pub mod handlers {
        use super::SyscallResult;
CHANGE
        #[test]
        fn credentials_commit_to_siblings_and_refusal_preserves_identity() {
            *crate::process::manager() = Some(crate::manager());
            assert_eq!(change_credentials(|c| c.setgid(1000)), SyscallResult::Ok(0));
            assert_eq!(change_credentials(|c| c.setuid(1000)), SyscallResult::Ok(0));
            for pid in [10, 11] {
                let state = crate::process::manager();
                let c = &state.as_ref().unwrap().processes.live_row(&crate::ProcessId(pid)).unwrap().cred;
                assert_eq!((c.uid, c.euid, c.gid, c.egid), (1000, 1000, 1000, 1000));
                assert_eq!(state.as_ref().unwrap().processes.live_row(&crate::ProcessId(pid)).unwrap().published_identity.get(), (1000, 1000, 1000));
            }
            assert_eq!(change_credentials(|c| c.setuid(0)), SyscallResult::Err(1));
            let state = crate::process::manager();
            assert_eq!(state.as_ref().unwrap().processes.live_row(&crate::ProcessId(11)).unwrap().cred.euid, 1000);
            assert_eq!(state.as_ref().unwrap().processes.live_row(&crate::ProcessId(30)).unwrap().cred.euid, 0);
        }
    }
}
#[test]
fn waits_include_sibling_children_and_respect_wnothread() {
    let mut m = manager();
    m.processes.live_row_mut(&ProcessId(10)).unwrap().terminated = true;
    assert_eq!(m.wait_children(ProcessId(11), false).unwrap(), [ProcessId(11), ProcessId(20), ProcessId(21)]);
    assert_eq!(m.wait_children(ProcessId(11), true).unwrap(), [ProcessId(21)]);
}
#[test]
fn children_remain_in_group_when_spawning_thread_exits() {
    let mut m = manager();
    m.processes.live_row_mut(&ProcessId(10)).unwrap().terminated = true;
    assert!(m.reparent_children_to_init(ProcessId(10), &[ProcessId(11), ProcessId(20)]));
    assert_eq!(m.processes.live_row(&ProcessId(20)).unwrap().parent, Some(ProcessId(11)));
    assert_ne!(m.processes.live_row(&ProcessId(11)).unwrap().parent, Some(ProcessId(11)));
    assert!(m.wait_children(ProcessId(11), true).unwrap().contains(&ProcessId(20)));
    m.processes.live_row_mut(&ProcessId(11)).unwrap().terminated = true;
    assert!(m.reparent_children_to_init(ProcessId(11), &[ProcessId(20), ProcessId(21)]));
    assert_eq!(m.processes.live_row(&ProcessId(20)).unwrap().parent, Some(ProcessId(1)));
}
'''
harness = harness.replace("CREDENTIALS_PATH", str(ROOT / "kernel/src/process/credentials.rs")).replace("METHODS", methods).replace("CHANGE", change)
with tempfile.TemporaryDirectory(prefix="breenix-process-group-") as tmp:
    test = Path(tmp) / "tests.rs"
    test.write_text(harness)
    binary = Path(tmp) / "tests"
    env = dict(os.environ, CARGO_BUILD_JOBS="4")
    subprocess.run(["rustc", "--edition=2021", "--test", str(test), "-o", str(binary)], cwd=ROOT, env=env, check=True)
    subprocess.run([str(binary), "--test-threads=1"], check=True)
