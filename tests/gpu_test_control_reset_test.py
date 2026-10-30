"""Execute DISARM_ALL's production arm for both architecture selections."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class DisarmGauge(unittest.TestCase):
    def test_disarm_resets_gpu_and_window_gauges_on_both_architectures(self):
        source = (ROOT / 'kernel/src/test_control.rs').read_text()
        arm = item(source, '        tc_op::DISARM_ALL =>')
        body = arm[arm.index('{'):]
        reset = item((ROOT / 'kernel/src/tracing/providers/gpu.rs').read_text(),
                     'pub fn reset_irq_masked(')
        harness = r'''
use std::sync::atomic::{AtomicU64,AtomicBool,Ordering};
struct Point(AtomicBool);
impl Point { fn disarm(&self) { self.0.store(false,Ordering::Relaxed); } }
static POINT_TABLE:[Point;2]=[Point(AtomicBool::new(true)),Point(AtomicBool::new(true))];
#[derive(Debug,PartialEq)] enum SyscallResult { Ok(u64) }
mod tracing { pub mod providers {
 pub mod gpu {
  use crate::{AtomicU64,Ordering};
  struct Slot {value:AtomicU64}
  struct Counter {per_cpu:[Slot;1]}
  static GPU_IRQ_MASKED_MAX_US:Counter=Counter {per_cpu:[Slot {value:AtomicU64::new(2000)}]};
  RESET
  pub fn maximum()->u64 {GPU_IRQ_MASKED_MAX_US.per_cpu[0].value.load(Ordering::Relaxed)}
 }
 pub mod window {
  static MAX:crate::AtomicU64=crate::AtomicU64::new(2000);
  pub fn reset_irq_masked(){MAX.store(0,crate::Ordering::Relaxed);}
  pub fn maximum()->u64{MAX.load(crate::Ordering::Relaxed)}
 }
}}
fn disarm()->SyscallResult BODY
fn main() {
 let _reset:fn()=tracing::providers::gpu::reset_irq_masked;
 assert_eq!(disarm(),SyscallResult::Ok(0));
 assert!(POINT_TABLE.iter().all(|p|!p.0.load(Ordering::Relaxed)));
 assert_eq!(tracing::providers::gpu::maximum(),0,"DISARM_ALL retained the armed 2ms hold");
 assert_eq!(tracing::providers::window::maximum(),0);
}
'''.replace('RESET', reset).replace('BODY', body)
        prefix = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            for arch in ['x86_64', 'aarch64']:
                with self.subTest(arch=arch):
                    selected = harness
                    for target in ['x86_64', 'aarch64']:
                        selected = selected.replace(f'target_arch = "{target}"',
                                                    'all()' if target == arch else 'any()')
                    (path / 'disarm.rs').write_text(selected)
                    subprocess.run(prefix + ['rustc', '--edition=2021', '-Dwarnings',
                                   str(path / 'disarm.rs'), '-o', str(path / 'disarm')], check=True)
                    subprocess.run([str(path / 'disarm')], check=True)


if __name__ == '__main__':
    unittest.main()
