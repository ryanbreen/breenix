"""Drive begin_fork with a process table that models CLONE_VM ownership."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def method(source, first, following):
    start = source.index(f"fn {first}(")
    end = source.index(f"\n    }}", start) + len("\n    }")
    assert end < source.index(f"fn {following}(", end)
    return source[start:end]


HARNESS = r'''
extern crate alloc;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ProcessId(u64);
impl ProcessId {
    fn as_u64(self) -> u64 { self.0 }
}

#[derive(Default)]
struct Process {
    main_thread: Option<String>,
    page_table: Option<Box<ProcessPageTable>>,
    inherited_cr3: Option<u64>,
    address_space_lifetime: Option<Arc<()>>,
    vmas: Vec<u64>,
    name: String,
    entry_point: u64,
    exe_path: Option<String>,
    cmdline: Option<String>,
    cmdline_area: u64,
    parent: Option<ProcessId>,
    pgid: u64,
    cwd: Cwd,
    fd_table: Fds,
    clear_child_tid: Option<u64>,
    heap_start: u64,
    heap_end: u64,
    mmap_hint: u64,
    memory_usage: MemoryUsage,
    user_stack_top: u64,
    user_stack_bottom: u64,
}
impl Process {
    fn is_tombstone(&self) -> bool { false }
    fn fork_child(_: ProcessId, name: String, entry_point: u64, _: &Self) -> Self {
        Self { name, entry_point, ..Self::default() }
    }
}
#[derive(Default)]
struct Cwd(u64);
impl Cwd {
    fn share(&self) -> Self { self.copy() }
    fn copy(&self) -> Self { Self(self.0) }
}
#[derive(Default)]
struct Fds(u64);
impl Fds { fn share(&self) -> Self { Self(self.0) } }
#[derive(Default)]
struct MemoryUsage { code_size: u64, heap_size: u64, stack_size: u64 }

struct ProcessPageTable(u64);
impl ProcessPageTable {
    fn level_4_frame(&self) -> &Self { self }
    fn start_address(&self) -> &Self { self }
    fn as_u64(&self) -> u64 { self.0 }
    fn address_space(&self) -> u64 { self.0 }
}
struct ProcessManager { processes: BTreeMap<ProcessId, Process> }
trait LiveRow {
    fn live_row(&self, pid: &ProcessId) -> Option<&Process>;
}
impl LiveRow for BTreeMap<ProcessId, Process> {
    fn live_row(&self, pid: &ProcessId) -> Option<&Process> { self.get(pid) }
}

struct SharedUndo(Option<ProcessId>);
struct ForkCopy {
    parent_pid: ProcessId, child_pid: ProcessId, parent_thread: String,
    child_process: Process, vmas: Vec<u64>, skip: Vec<(u64, u64)>,
    child_page_table: Box<ProcessPageTable>, parent_page_table: *const ProcessPageTable,
    claim: memory::fork_copy::CopyClaim, undo: SharedUndo,
}
mod memory {
    pub mod process_memory { pub type UnpublishedPageTable = Box<crate::ProcessPageTable>; }
    pub mod file_map {
        pub fn fork_vmas(
            vmas: &[u64], _: crate::ProcessId, _: &crate::ProcessPageTable,
        ) -> Result<Vec<u64>, &'static str> { Ok(vmas.to_vec()) }
        pub fn dontcopy_ranges(vmas: &[u64]) -> Result<Vec<(u64, u64)>, &'static str> {
            Ok(vmas.iter().map(|start| (*start, start + 4096)).collect())
        }
    }
    pub mod fork_copy {
        pub struct CopyClaim(pub u64, pub u64);
        impl CopyClaim {
            pub fn take(root: u64, child: u64) -> Self { Self(root, child) }
        }
    }
}
mod syscall {
    pub mod clone {
        #[derive(Default)]
        pub struct ForkClone {
            pub share_fs: bool, pub share_files: bool,
            pub share_undo: bool, pub child_clear_tid: Option<u64>,
        }
    }
    pub mod exec {
        pub fn copy_string(name: &str) -> Result<String, ()> { Ok(name.to_owned()) }
    }
}
mod ipc { pub mod sysv {
    pub fn share_undo(_: crate::ProcessId, _: crate::ProcessId) -> Result<(), ()> { Ok(()) }
} }
mod tty { pub mod ctty {
    pub fn inherit(_: &mut crate::Process, _: &crate::Process) {}
} }
mod tracing { pub mod providers { pub mod process {
    pub fn trace_fork_entry(_: u32) {}
} } }

#[test]
fn worker_forks_with_its_shared_address_space_and_own_context() {
    let root = 0x1000;
    let owner = ProcessId(11);
    let worker = ProcessId(12);
    let lifetime = Arc::new(());
    let mut manager = ProcessManager { processes: BTreeMap::from([
        (owner, Process {
            page_table: Some(Box::new(ProcessPageTable(root))),
            address_space_lifetime: Some(lifetime.clone()),
            vmas: vec![0x3000],
            heap_end: 0x4000,
            user_stack_top: 0x8000,
            ..Process::default()
        }),
        (worker, Process {
            main_thread: Some("worker context".to_owned()),
            inherited_cr3: Some(root),
            address_space_lifetime: Some(lifetime),
            cwd: Cwd(6),
            fd_table: Fds(7),
            ..Process::default()
        }),
    ]) };
    assert_eq!(manager.find_process_by_cr3(root).unwrap().0, owner);
    let caller = &manager.processes[&worker];
    assert_eq!(caller.inherited_cr3, Some(root));
    assert!(Arc::ptr_eq(
        caller.address_space_lifetime.as_ref().unwrap(),
        manager.processes[&owner].address_space_lifetime.as_ref().unwrap(),
    ));
    let flags = syscall::clone::ForkClone { share_files: true, ..Default::default() };
    let fork = manager.begin_fork(worker, Box::new(ProcessPageTable(0x2000)), &flags)
        .expect("fork must resolve the worker's inherited address space");
    assert_eq!(fork.parent_pid, worker);
    assert_eq!(fork.parent_thread, "worker context");
    assert_eq!(fork.child_process.parent, Some(worker));
    assert_eq!(fork.child_process.cwd.0, 6);
    assert_eq!(fork.child_process.fd_table.0, 7);
    assert_eq!(fork.child_process.heap_end, 0x4000);
    assert_eq!(fork.child_process.user_stack_top, 0x8000);
    assert_eq!(fork.vmas, [0x3000]);
    assert_eq!(fork.skip, [(0x3000, 0x4000)]);
    assert_eq!(fork.parent_page_table, manager.processes[&owner].page_table.as_deref().unwrap()
        as *const ProcessPageTable);
    assert_eq!(fork.claim.0, root);
    assert_eq!(fork.claim.1, fork.child_page_table.0);
    assert_eq!(fork.child_pid, ProcessId(20));
    assert!(fork.undo.0.is_none());
}
'''


class SharedAddressSpaceFork(unittest.TestCase):
    def test_worker_fork_uses_the_shared_address_space(self):
        source = (ROOT / "kernel/src/process/manager.rs").read_text()
        owner_lookup = method(source, "find_process_by_cr3", "find_process_by_cr3_mut")
        begin_fork = method(source, "begin_fork", "end_fork_copy")
        with tempfile.TemporaryDirectory(prefix="breenix-fork-host-") as temp:
            path = Path(temp) / "fork.rs"
            path.write_text(HARNESS + f"""
impl ProcessManager {{
    fn allocate_ordinary_pid(&mut self) -> ProcessId {{ ProcessId(20) }}
    {owner_lookup}
    {begin_fork}
}}
""")
            binary = path.with_suffix("")
            subprocess.run(["rustc", "--edition=2021", "-Dwarnings", "--test", path, "-o", binary],
                           cwd=ROOT, check=True, timeout=30)
            subprocess.run([binary], check=True, timeout=10)


if __name__ == "__main__":
    unittest.main()
