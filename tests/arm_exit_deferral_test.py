"""A dead ARM address space must not be walked before publishing its zombie."""
from pathlib import Path
import unittest

from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class ArmExitDeferral(unittest.TestCase):
    def test_exit_hands_large_unmapped_root_to_bounded_reclaimer(self):
        source = (ROOT / 'kernel/src/task/process_task.rs').read_text()
        start = source.index('                    let retirement_receipt: Option<')
        end = source.index('                    // Keep termination after release/deferral.', start)
        branch = source[start:end].replace(
            '#[cfg(target_arch = "aarch64")]', '#[cfg(all())]'
        ).replace('#[cfg(target_arch = "x86_64")]', '#[cfg(any())]')
        run_rust(self, r'''
use std::sync::atomic::{AtomicUsize, Ordering};
static WALKED: AtomicUsize = AtomicUsize::new(0);
struct Process { live_root: bool, pages: usize, receipts: usize }
struct Pending { pages: usize }
mod process {
    pub struct RetirementReceipt(pub super::Pending);
    impl RetirementReceipt {
        pub fn from_reclaim(pending: super::Pending) -> Self { Self(pending) }
    }
}
fn defer_process_resources(process: &mut Process) -> Pending {
    process.receipts += 1;
    Pending { pages: std::mem::take(&mut process.pages) }
}
fn defer_remaining_tables(process: &mut Process) -> Option<Pending> {
    (process.pages != 0).then(|| defer_process_resources(process))
}
fn defer_live_process_resources(process: &mut Process) -> Option<Pending> {
    process.live_root.then(|| defer_process_resources(process))
}
fn release_process_leaves(process: &mut Process) {
    WALKED.fetch_add(process.pages, Ordering::Relaxed);
}
fn exit(process: &mut Process, already_terminated: bool, deferred_release: bool)
    -> Option<process::RetirementReceipt> {
    @BRANCH@
    retirement_receipt
}
#[test]
fn zombie_publication_does_not_wait_for_leaf_release() {
    // Keep historical helpers type-checked when the production branch stops using them.
    let _ = (defer_live_process_resources, release_process_leaves);
    for live_root in [false, true] {
        let mut process = Process { live_root, pages: 32768, receipts: 0 };
        let pending = exit(&mut process, false, false).expect("lost retirement receipt").0;
        assert_eq!(WALKED.load(Ordering::Relaxed), 0,
            "ARM exit walked mapped leaves before its row became waitable");
        assert_eq!((process.pages, process.receipts, pending.pages), (0, 1, 32768),
            "receipt must retain the leaf custody for the existing bounded drain");
        assert!(exit(&mut process, true, false).is_none(), "repeat exit issued another receipt");
        assert_eq!(process.receipts, 1);
    }
}
'''.replace('@BRANCH@', branch))


if __name__ == '__main__':
    unittest.main()
