"""Run the production owner handoff at both exit sites as an x86 exit."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def handoff(source):
    lines = source.splitlines()
    decl = next(i for i, line in enumerate(lines) if 'let mut gpu_owner = None;' in line)
    assign = next(i for i, line in enumerate(lines) if 'gpu_owner = crate::gpu::exited_locked(' in line)
    redeem = next(i for i, line in enumerate(lines) if 'if let Some(owner) = gpu_owner {' in line)

    def guarded(first, last):
        if lines[first - 1].strip().startswith('#[cfg('):
            first -= 1
        # The selected production sites previously admitted ARM only. The host
        # harness models x86 by making that architecture condition false.
        return '\n'.join(lines[first:last]).replace('#[cfg(target_arch = "aarch64")]', '#[cfg(any())]')

    return guarded(decl, decl + 1), guarded(assign - 1, assign + 2), guarded(redeem, redeem + 3)


class ExitOwner(unittest.TestCase):
    def test_x86_exit_delivers_owner_outside_the_process_lock(self):
        harness = r'''
use std::sync::atomic::{AtomicBool,AtomicU64,Ordering};
static LOCKED:AtomicBool=AtomicBool::new(false);
static RELEASED:AtomicU64=AtomicU64::new(0);
mod gpu {
 pub fn exited_locked(_: &(),pid:u64)->Option<u64> {
  assert!(super::LOCKED.load(super::Ordering::SeqCst));
  (pid!=0).then_some(pid)
 }
 pub fn owner_ended(owner:u64) {
  assert!(!super::LOCKED.load(super::Ordering::SeqCst));
  super::RELEASED.store(owner,super::Ordering::SeqCst);
 }
}
fn exit(pid:u64) {
 // DECL
 let pm=&();let manager=pm;let _=manager;
 LOCKED.store(true,Ordering::SeqCst);
 // ASSIGN
 LOCKED.store(false,Ordering::SeqCst);
 // REDEEM
 let _=pid;
}
fn main(){
 let _:fn(&(),u64)->Option<u64>=gpu::exited_locked;
 let _:fn(u64)=gpu::owner_ended;
 exit(261);
 assert_eq!(RELEASED.load(Ordering::SeqCst),261,"x86 exit left GPU owner live until the periodic sweep");
 exit(0);assert_eq!(RELEASED.load(Ordering::SeqCst),261);
}
'''
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        for name in ['kernel/src/process/mod.rs', 'kernel/src/task/process_task.rs']:
            with self.subTest(site=name), tempfile.TemporaryDirectory() as tmp:
                declaration, assignment, redemption = handoff((ROOT / name).read_text())
                fixture = harness.replace('// DECL', declaration).replace('// ASSIGN', assignment).replace('// REDEEM', redemption)
                path = Path(tmp)
                (path / 'exit.rs').write_text(fixture)
                subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(path / 'exit.rs'), '-o', str(path / 'exit')], check=True)
                subprocess.run([str(path / 'exit')], check=True)


if __name__ == '__main__':
    unittest.main()
