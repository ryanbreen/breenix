"""Run syscall 416's real dispatch arm, provider and first-enable seam on a host clock."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class SyscallMaskSpan(unittest.TestCase):
    def test_inherited_mask_includes_lock_and_pri_until_enable_or_return(self):
        source = (ROOT / 'kernel/src/syscall/mod.rs').read_text()
        arm = source.split('        gpu_abi::SYS_GPU =>', 1)[1].split('        _ => return None,', 1)[0].strip().rstrip(',')
        enable = item((ROOT / 'kernel/src/gpu/arch.rs').read_text(),
                      'pub(super) fn with_interrupts<').replace('pub(super)', 'pub')
        provider = (ROOT / 'kernel/src/tracing/providers/gpu.rs').read_text()
        provider = '\n'.join(line for line in provider.splitlines() if not line.startswith('//!'))
        fixture = r'''
extern crate alloc;
use std::cell::Cell;
use std::sync::atomic::{AtomicU32, Ordering};
thread_local! {
 static CLOCK:Cell<u64>=const{Cell::new(100)};
 static READS:Cell<u64>=const{Cell::new(0)};
 static IRQS:Cell<bool>=const{Cell::new(false)};
 static CPU:Cell<usize>=const{Cell::new(0)};
}
fn advance(us:u64){CLOCK.with(|c|c.set(c.get()+us));}
fn arch_read_timestamp()->u64{READS.with(|r|r.set(r.get()+1));CLOCK.with(Cell::get)}
fn arch_ticks_to_nanos(ticks:u64)->u64{ticks*1_000}
fn arch_interrupts_enabled()->bool{IRQS.with(Cell::get)}
unsafe fn arch_enable_interrupts(){IRQS.with(|i|i.set(true));}
unsafe fn arch_disable_interrupts(){IRQS.with(|i|i.set(false));}
fn arch_without_interrupts<R>(f:impl FnOnce()->R)->R{f()}
pub mod task {pub mod scheduler {pub const MAX_CPUS:usize=2;}}
pub mod signal {pub struct ThreadSignals {pub gpu_path:crate::AtomicU32}}
pub struct Thread {pub signals:alloc::sync::Arc<signal::ThreadSignals>}
pub mod per_cpu {
 pub fn cpu_id()->usize{crate::CPU.with(std::cell::Cell::get)}
 pub fn is_initialized()->bool{true}
 pub fn in_interrupt()->bool{false}
 pub fn current_thread()->Option<&'static crate::Thread>{None}
}
pub mod tracing {
 pub mod counter {
  use std::sync::atomic::AtomicU64;
  pub struct Slot {pub value:AtomicU64}
  pub struct TraceCounter {pub per_cpu:[Slot;2]}
  impl TraceCounter {pub const fn new(_: &str,_: &str)->Self {
   Self{per_cpu:[const{Slot{value:AtomicU64::new(0)}};2]}
  }}
  pub fn register_counter(_: &TraceCounter)->Option<usize>{Some(0)}
 }
 pub mod providers {pub mod gpu {PROVIDER}}
}
mod gpu_arch {ENABLE}
#[derive(Debug,PartialEq)]enum SyscallResult {Ok(u64),Err(u64)}
mod gpu {
 pub fn sys_gpu(op:u64,_arg:u64,_len:u64)->crate::SyscallResult {
  assert!(!crate::arch_interrupts_enabled());
  // Time before CORE acquisition, then the adapter's PRI retry under that lock.
  crate::advance(300);
  crate::advance(350);
  if op==0 {return crate::SyscallResult::Err(22);}
  crate::gpu_arch::with_interrupts(|| {
   assert!(crate::arch_interrupts_enabled());
   crate::advance(20_000);
  });
  assert!(!crate::arch_interrupts_enabled());
  crate::advance(10);
  // A later enable must not close the inherited interval a second time.
  crate::gpu_arch::with_interrupts(||crate::advance(30_000));
  crate::SyscallResult::Ok(7)
 }
}
fn dispatch(a:[u64;6])->SyscallResult {DISPATCH_ARM}
fn maximum()->u64 {
 tracing::providers::gpu::GPU_IRQ_MASKED_MAX_US.per_cpu[0].value.load(Ordering::Relaxed)
}
#[test]fn inherited_dispatch_span() {
 for op in [0,1] {
  tracing::providers::gpu::reset_irq_masked();READS.with(|r|r.set(0));
  assert_eq!(dispatch([op,0,0,0,0,0]),if op==0{SyscallResult::Err(22)}else{SyscallResult::Ok(7)});
  assert_eq!(maximum(),650,"the gauge must include the contiguous 650 us inherited mask, excluding IRQ-enabled work");
  assert_eq!(READS.with(Cell::get),2,"one counter read at each endpoint, with no repeat at a later enable or return");
  assert!(!arch_interrupts_enabled());
 }
 tracing::providers::gpu::reset_irq_masked();assert_eq!(maximum(),0);
 // A has enabled and migrated; its eventual return must leave B's new
 // inherited interval on A's original CPU intact.
 let a=tracing::providers::gpu::X86SyscallMask::enter();
 advance(12);gpu_arch::with_interrupts(||());
 let b=tracing::providers::gpu::X86SyscallMask::enter();
 CPU.with(|c|c.set(1));drop(a);
 CPU.with(|c|c.set(0));advance(800);drop(b);
 assert_eq!(maximum(),800,"a migrated return must not consume another syscall's start");
 tracing::providers::gpu::reset_irq_masked();assert_eq!(maximum(),0);
}
'''.replace('DISPATCH_ARM', arm).replace('ENABLE', enable).replace('PROVIDER', provider)
        # Execute the x86 production selection on either host architecture.
        fixture = fixture.replace('target_arch = "x86_64"', 'all()')
        fixture = fixture.replace('target_arch = "aarch64"', 'any()')
        prefix = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'span.rs').write_text(fixture)
            subprocess.run(prefix + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                           str(path / 'span.rs'), '-o', str(path / 'span')], check=True)
            subprocess.run([str(path / 'span')], check=True)


if __name__ == '__main__':
    unittest.main()
