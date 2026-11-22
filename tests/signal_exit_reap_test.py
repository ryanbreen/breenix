"""Drive the kernel's signal exit decision against both wait/cleanup orders."""
from pathlib import Path
import unittest

from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


def item(source, signature):
    start = source.index(signature)
    end = '\n' + signature[:len(signature) - len(signature.lstrip())] + '}'
    return source[start:source.index(end, start) + len(end)]


class SignalExitReap(unittest.TestCase):
    def test_deferred_signal_exit_releases_all_row_resources(self):
        signal = (ROOT / 'kernel/src/signal/delivery.rs').read_text()
        process = (ROOT / 'kernel/src/process/process.rs').read_text()
        manager = (ROOT / 'kernel/src/process/manager.rs').read_text()
        lookup = item(manager, '    pub fn find_process_by_thread(').removesuffix(
            '\n    }'
        ).rsplit('self.processes', 1)[1]
        model = r'''
use std::cell::Cell;
use std::rc::Rc;

extern crate self as log;
macro_rules! debug { ($($argument:tt)*) => {}; }
pub(crate) use debug;
macro_rules! trace_count { ($($argument:tt)*) => {}; }
pub(crate) use trace_count;

type ProcessId = u64;
struct Thread { id: u64 }
impl Thread { fn id(&self) -> u64 { self.id } }
struct Process {
    main_thread: Option<Thread>,
    unqueued_fatal_exit: Option<i32>,
    terminated: bool,
    reaped: Option<(ProcessId, i32)>,
    dirs: Option<(Rc<()>, Rc<()>)>,
    files: Option<Rc<()>>,
    mappings: Option<Rc<()>>,
    free_gaps: Option<Rc<()>>,
}
impl Process {
    fn is_terminated(&self) -> bool { self.terminated }
    fn is_tombstone(&self) -> bool { self.reaped.is_some() }
    fn terminate(&mut self, _: i32) {
        self.files.take();
        self.terminated = true;
    }
    @CLAIM_REAP@
}

enum SignalDefaultAction { Terminate, CoreDump, Stop, Continue, Ignore }
fn default_action(sig: u32) -> SignalDefaultAction {
    match sig {
        15 => SignalDefaultAction::Terminate,
        11 => SignalDefaultAction::CoreDump,
        19 => SignalDefaultAction::Stop,
        18 => SignalDefaultAction::Continue,
        _ => SignalDefaultAction::Ignore,
    }
}
type ParentNotification = ();
fn notify_parent_of_termination(_: &Process) -> Option<ParentNotification> { Some(()) }
@FATAL_EXIT_CODE@
fn signal_death_exit_code(_: &Process, sig: u32) -> i32 { fatal_exit_code(sig).unwrap() }
mod ipc { pub mod sysv { pub fn row_dying_locked(_: &mut crate::Process) {} } }
mod task {
    pub mod scheduler { pub fn terminate_thread(_: u64) {} }
    pub mod process_task {
        pub fn defer_fault_exit(_: u64, code: i32) -> bool {
            if crate::FULL.get() { return false; }
            crate::QUEUED.set(Some(code));
            true
        }
        pub fn note_unqueued_fatal_exit() {}
    }
}
thread_local! {
    static FULL: Cell<bool> = const { Cell::new(false) };
    static QUEUED: Cell<Option<i32>> = const { Cell::new(None) };
}
@RESULT@
@DELIVER@

fn clean_up(processes: &mut [(ProcessId, Process)], thread_id: u64) {
    // Use the worker's thread lookup predicate. The effects below model
    // handle_thread_exit's cwd/root release, fd close and address space retirement.
    let pid = processes @LOOKUP@ .map(|(pid, _)| pid);
    if let Some(pid) = pid {
        let process = &mut processes.iter_mut().find(|(id, _)| id == &pid).unwrap().1;
        process.dirs.take();
        process.files.take();
        process.mappings.take();
        process.free_gaps.take();
        process.terminated = true;
    }
}

#[test]
fn both_orderings_release_every_resource() {
    let mut failures = vec![];
    for (sig, status) in [(15, -15), (11, -139)] {
        for full in [false, true] {
            for reap_first in [false, true] {
                let resources = ["cwd", "root", "open files", "mappings", "free-gap index"];
                let [cwd, root, files, mappings, free_gaps] = resources.map(|_| Rc::new(()));
                let refs = [&cwd, &root, &files, &mappings, &free_gaps].map(Rc::downgrade);
                let mut rows = [(2, Process {
                    main_thread: Some(Thread { id: 3 }), unqueued_fatal_exit: None,
                    terminated: false, reaped: None,
                    dirs: Some((cwd, root)), files: Some(files),
                    mappings: Some(mappings), free_gaps: Some(free_gaps),
                })];
                FULL.set(full);
                deliver_default_action(&mut rows[0].1, sig);
                if reap_first { rows[0].1.claim_reap(1, status); }
                let queued = QUEUED.take().or_else(|| rows[0].1.unqueued_fatal_exit.take());
                assert_eq!(queued, Some(status), "the deferred worker must receive the exit");
                clean_up(&mut rows, 3);
                if rows[0].1.reaped.is_none() { assert!(rows[0].1.claim_reap(1, status)); }
                assert!(!rows[0].1.claim_reap(1, status));
                let held: Vec<_> = resources.into_iter().zip(refs)
                    .filter_map(|(name, held)| held.upgrade().is_some().then_some(name)).collect();
                if !held.is_empty() {
                    failures.push(format!("signal={sig} queue_full={full} reap_first={reap_first}: {held:?}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "row retained resources: {}", failures.join("\n"));
}
'''
        for marker, source in {
            'CLAIM_REAP': item(process, '    pub(crate) fn claim_reap('),
            'LOOKUP': lookup,
            'FATAL_EXIT_CODE': item(signal, 'pub fn fatal_exit_code('),
            'RESULT': item(signal, 'pub enum DeliverResult'),
            'DELIVER': item(signal, 'fn deliver_default_action('),
        }.items():
            model = model.replace(f'@{marker}@', source)
        for arch, other in (('aarch64', 'x86_64'), ('x86_64', 'aarch64')):
            with self.subTest(arch=arch):
                run_rust(self, model.replace(f'target_arch = "{arch}"', 'all()').replace(
                    f'target_arch = "{other}"', 'any()'
                ))


if __name__ == '__main__':
    unittest.main()
