"""Drive fork preparation and address-space handoff with modeled process rows."""
import json
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
    image_size: u64,
    image_data_size: u64,
    user_stack_top: u64,
    user_stack_bottom: u64,
    terminated: bool,
    memory_locks: Vec<u64>,
    pending_old_page_tables: Vec<Box<ProcessPageTable>>,
    fb_mmap: Option<u64>,
    mmap_pending: Vec<u64>,
    mmap_gaps: Option<u64>,
    address_space_release_deferred: bool,
}
impl Process {
    fn is_tombstone(&self) -> bool { false }
    fn is_terminated(&self) -> bool { self.terminated }
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
    fn live_row_mut(&mut self, pid: &ProcessId) -> Option<&mut Process>;
}
impl LiveRow for BTreeMap<ProcessId, Process> {
    fn live_row(&self, pid: &ProcessId) -> Option<&Process> { self.get(pid) }
    fn live_row_mut(&mut self, pid: &ProcessId) -> Option<&mut Process> { self.get_mut(pid) }
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
    // Compile the real claim module. This fixture drives its fork path.
    #[allow(dead_code)]
    #[path = FORK_COPY_PATH]
    pub(crate) mod fork_copy;
}
mod task { pub mod scheduler {
    pub const MAX_CPUS: usize = 1;
    pub fn current_cpu() -> usize { 0 }
} }
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
            image_size: 0x5000,
            image_data_size: 0x1000,
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
    assert_eq!(fork.child_process.image_size, 0x5000);
    assert_eq!(fork.child_process.image_data_size, 0x1000);
    assert_eq!(fork.child_process.user_stack_top, 0x8000);
    assert_eq!(fork.vmas, [0x3000]);
    assert_eq!(fork.skip, [(0x3000, 0x4000)]);
    assert_eq!(fork.parent_page_table, manager.processes[&owner].page_table.as_deref().unwrap()
        as *const ProcessPageTable);
    assert!(memory::fork_copy::copying(root));
    assert!(memory::fork_copy::forming(fork.child_page_table.0));
    assert_eq!(fork.child_pid, ProcessId(20));
    assert!(fork.undo.0.is_none());
    drop(fork.claim);
}

#[test]
fn owner_exit_hands_off_the_table_during_this_cpus_fork_copy() {
    let root = 0x1000;
    let child_root = 0x2000;
    let owner = ProcessId(11);
    let worker = ProcessId(12);
    let lifetime = Arc::new(());
    let table = Box::new(ProcessPageTable(root));
    let parent_table = std::ptr::from_ref(table.as_ref());
    let mut manager = ProcessManager { processes: BTreeMap::from([
        (owner, Process {
            page_table: Some(table),
            address_space_lifetime: Some(lifetime.clone()),
            ..Process::default()
        }),
        (worker, Process {
            main_thread: Some("worker context".to_owned()),
            inherited_cr3: Some(root),
            address_space_lifetime: Some(lifetime),
            ..Process::default()
        }),
    ]) };
    let mut fork = manager.begin_fork(
        worker, Box::new(ProcessPageTable(child_root)), &Default::default(),
    ).expect("worker must claim its shared root");
    assert!(memory::fork_copy::copying(root));

    assert!(manager.transfer_address_space(owner));
    assert!(manager.processes[&owner].page_table.is_none());
    let (heir, row) = manager.find_process_by_cr3(root).expect("root must still have an owner");
    assert_eq!(heir, worker);
    assert_eq!(std::ptr::from_ref(row.page_table.as_deref().unwrap()), parent_table);
    assert!(memory::fork_copy::copying(root));
    assert!(memory::fork_copy::forming(child_root));
    fork.claim.end_copy();
    assert!(!memory::fork_copy::copying(root));
}

#[test]
fn single_owner_forks_its_own_address_space() {
    let root = 0x1000;
    let owner = ProcessId(11);
    let mut manager = ProcessManager { processes: BTreeMap::from([
        (owner, Process {
            main_thread: Some("owner context".to_owned()),
            page_table: Some(Box::new(ProcessPageTable(root))),
            vmas: vec![0x3000],
            ..Process::default()
        }),
    ]) };
    let fork = manager.begin_fork(
        owner, Box::new(ProcessPageTable(0x2000)), &Default::default(),
    ).expect("a single owner can fork");
    assert_eq!(fork.parent_pid, owner);
    assert_eq!(fork.parent_thread, "owner context");
    assert_eq!(fork.child_process.parent, Some(owner));
    assert_eq!(fork.vmas, [0x3000]);
    assert!(memory::fork_copy::copying(root));
}

#[test]
fn inherited_root_from_a_different_lifetime_is_refused() {
    let root = 0x1000;
    let child_root = 0x2000;
    let owner = ProcessId(11);
    let worker = ProcessId(12);
    let mut manager = ProcessManager { processes: BTreeMap::from([
        (owner, Process {
            page_table: Some(Box::new(ProcessPageTable(root))),
            address_space_lifetime: Some(Arc::new(())),
            ..Process::default()
        }),
        (worker, Process {
            main_thread: Some("worker context".to_owned()),
            inherited_cr3: Some(root),
            address_space_lifetime: Some(Arc::new(())),
            ..Process::default()
        }),
    ]) };
    assert_eq!(manager.find_process_by_cr3(root).unwrap().0, owner);
    let result = manager.begin_fork(
        worker, Box::new(ProcessPageTable(child_root)), &Default::default(),
    );
    assert_eq!(result.err(), Some("Parent process has no page table"));
    assert!(!memory::fork_copy::copying(root));
    assert!(!memory::fork_copy::forming(child_root));
}
'''


class SharedAddressSpaceFork(unittest.TestCase):
    def test_fork_and_address_space_handoff(self):
        source = (ROOT / "kernel/src/process/manager.rs").read_text()
        owner_lookup = method(source, "find_process_by_cr3", "find_process_by_cr3_mut")
        begin_fork = method(source, "begin_fork", "end_fork_copy")
        transfer = method(source, "transfer_address_space", "remove_from_ready_queue")
        fork_copy = ROOT / "kernel/src/memory/fork_copy.rs"
        with tempfile.TemporaryDirectory(prefix="breenix-fork-host-") as temp:
            path = Path(temp) / "fork.rs"
            path.write_text(HARNESS.replace("FORK_COPY_PATH", json.dumps(str(fork_copy))) + f"""
impl ProcessManager {{
    fn allocate_ordinary_pid(&mut self) -> ProcessId {{ ProcessId(20) }}
    {owner_lookup}
    {begin_fork}
    {transfer}
}}
""")
            binary = path.with_suffix("")
            subprocess.run(["rustc", "--edition=2021", "-Dwarnings", "--test", path, "-o", binary],
                           cwd=ROOT, check=True, timeout=30)
            # Serialize the tests because the scheduler stub returns CPU 0.
            subprocess.run([binary, "--test-threads=1"], check=True, timeout=10)


if __name__ == "__main__":
    unittest.main()
