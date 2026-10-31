"""Exercise x86's production masking hooks with a deterministic host clock."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class X86Gauge(unittest.TestCase):
    def test_production_provider_times_gpu_scopes_inside_x86_syscall_mask(self):
        cpu = item((ROOT / 'kernel/src/arch_impl/x86_64/cpu.rs').read_text(), '    fn without_interrupts<')
        provider = (ROOT / 'kernel/src/tracing/providers/gpu.rs').read_text()
        mutex = (ROOT / 'kernel/src/irq_safe_mutex.rs').read_text().replace('use spin::', 'use crate::spin::')
        mutex = mutex.replace('target_arch = "aarch64"', 'any()').replace('target_arch = "x86_64"', 'all()')
        fixture = r'''
extern crate alloc;
use std::cell::Cell;
use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicU32, Ordering};
thread_local! {
 static IRQS:Cell<bool>=const{Cell::new(true)};
 static CLOCK:Cell<u64>=const{Cell::new(1)};
 static IN_INTERRUPT:Cell<bool>=const{Cell::new(false)};
 static INITIALIZED:Cell<bool>=const{Cell::new(true)};
}
fn arch_interrupts_enabled()->bool{IRQS.with(Cell::get)}
unsafe fn arch_disable_interrupts(){IRQS.with(|i|i.set(false));}
unsafe fn arch_enable_interrupts(){IRQS.with(|i|i.set(true));}
fn arch_read_timestamp()->u64{CLOCK.with(Cell::get)}
fn arch_ticks_to_nanos(ticks:u64)->u64{ticks*1_000}
fn advance(us:u64){CLOCK.with(|c|c.set(c.get()+us));}
fn arch_without_interrupts<R>(f:impl FnOnce()->R)->R{Cpu::without_interrupts(f)}
mod signal {pub struct ThreadSignals{pub gpu_path:crate::AtomicU32}}
pub mod task {pub mod scheduler {pub const MAX_CPUS:usize=1;}}
pub struct Thread{signals:Arc<signal::ThreadSignals>}
static THREAD:OnceLock<Thread>=OnceLock::new();
pub mod per_cpu {
 pub fn cpu_id()->usize{0}
 pub fn is_initialized()->bool{crate::INITIALIZED.with(std::cell::Cell::get)}
 pub fn in_interrupt()->bool{crate::IN_INTERRUPT.with(std::cell::Cell::get)}
 pub fn current_thread()->Option<&'static crate::Thread>{crate::THREAD.get()}
}
pub mod tracing {
 pub mod counter {
  use std::sync::atomic::AtomicU64;
  pub struct Slot{pub value:AtomicU64}
  pub struct TraceCounter{pub per_cpu:[Slot;1]}
  impl TraceCounter {pub const fn new(_: &str,_: &str)->Self{Self{per_cpu:[Slot{value:AtomicU64::new(0)}]}}}
  pub fn register_counter(_: &TraceCounter)->Option<usize>{Some(0)}
 }
 pub mod providers {pub mod gpu {PROVIDER}}
}
mod x86_64 {pub mod instructions {pub mod interrupts {
 pub fn without_interrupts<R>(f:impl FnOnce()->R)->R {
  let enabled=crate::arch_interrupts_enabled();unsafe{crate::arch_disable_interrupts();}
  let result=f();if enabled{unsafe{crate::arch_enable_interrupts();}}result
 }
}}}
mod spin {
 pub type MutexGuard<'a,T>=std::sync::MutexGuard<'a,T>;
 pub struct Mutex<T>(std::sync::Mutex<T>);
 impl<T> Mutex<T>{pub const fn new(v:T)->Self{Self(std::sync::Mutex::new(v))}
  pub fn lock(&self)->MutexGuard<'_,T>{self.0.lock().unwrap()}
  pub fn try_lock(&self)->Option<MutexGuard<'_,T>>{self.0.try_lock().ok()}}
}
mod irq_mutex {MUTEX}
struct Cpu;
impl Cpu {fn interrupts_enabled()->bool{arch_interrupts_enabled()} CPU}
use tracing::providers::gpu::*;
fn maximum()->u64{GPU_IRQ_MASKED_MAX_US.per_cpu[0].value.load(Ordering::Relaxed)}
#[test]fn gpu_path_uses_the_real_provider_and_the_x86_primitive(){
 init();
 for counter in [&GPU_CONTEXTS,&GPU_RESOURCES,&GPU_RESOURCE_BYTES,&GPU_VRAM_BYTES,&GPU_PRESENTS_SUSPENDED] {
  gauge(counter,1);gauge(counter,-1);
  assert_eq!(counter.per_cpu[0].value.load(Ordering::Relaxed),0);
 }
 THREAD.set(Thread{signals:Arc::new(signal::ThreadSignals{gpu_path:AtomicU32::new(0)})}).ok().unwrap();
 Cpu::without_interrupts(||advance(3_000));
 assert_eq!(maximum(),0,"non-GPU threads do not contribute");
 let path=GpuPath::enter();
 Cpu::without_interrupts(||{advance(3);Cpu::without_interrupts(||advance(7));advance(2);});
 assert!(arch_interrupts_enabled());assert_eq!(maximum(),12);
 reset_irq_masked();
 // SYSCALL/FMASK has already cleared IF when syscall 416 enters GpuPath.
 unsafe{arch_disable_interrupts();}
 Cpu::without_interrupts(||advance(2_000));
 assert!(!arch_interrupts_enabled());
 assert_eq!(maximum(),EXPECTED_HOLD,"the production provider must see the 2 ms GPU hold inside the inherited syscall mask");
 let m=irq_mutex::IrqSafeMutex::new(7);
 {let mut g=m.lock();*g=9;advance(3_000);assert!(m.try_lock().is_none());}
 assert!(!arch_interrupts_enabled());assert_eq!(maximum(),EXPECTED_MUTEX);
 unsafe{arch_enable_interrupts();}
 reset_irq_masked();
 {let g=m.try_lock().unwrap();assert_eq!(*g,9);advance(501);Cpu::without_interrupts(||advance(1));}
 assert!(arch_interrupts_enabled());assert_eq!(maximum(),502);
 Cpu::without_interrupts(||advance(1));assert_eq!(maximum(),502);
 reset_irq_masked();
 IN_INTERRUPT.with(|v|v.set(true));
 Cpu::without_interrupts(||advance(4_000));assert_eq!(maximum(),0);
 IN_INTERRUPT.with(|v|v.set(false));
 INITIALIZED.with(|v|v.set(false));
 Cpu::without_interrupts(||advance(4_000));assert_eq!(maximum(),0);
 INITIALIZED.with(|v|v.set(true));
 drop(path);
 Cpu::without_interrupts(||advance(4_000));assert_eq!(maximum(),0);
 let start=interrupt_begin();advance(5);interrupt_end(start);assert_eq!(maximum(),5);
 reset_irq_masked();assert_eq!(maximum(),0);
}
'''
        fixture = fixture.replace('{MUTEX}', '{\n' + '\n'.join(line for line in mutex.splitlines() if not line.startswith('//!')) + '\n}').replace(' CPU}', '\n' + cpu + '\n}')
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            for arch in ['x86_64', 'aarch64']:
                with self.subTest(provider_arch=arch):
                    # The CPU primitive remains x86; select the provider/mutex
                    # architecture explicitly on either kind of host.
                    selected = provider
                    for target in ['x86_64', 'aarch64']:
                        selected = selected.replace(f'target_arch = "{target}"', 'all()' if target == arch else 'any()')
                    source = fixture.replace('PROVIDER', '\n'.join(line for line in selected.splitlines() if not line.startswith('//!')))
                    source = source.replace('EXPECTED_HOLD', '2_000' if arch == 'x86_64' else '0').replace('EXPECTED_MUTEX', '3_000' if arch == 'x86_64' else '0')
                    (path / 'provider.rs').write_text(source)
                    subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                        str(path / 'provider.rs'), '-o', str(path / 'provider')], check=True)
                    subprocess.run([str(path / 'provider')], check=True)

    def test_nested_masks_and_mutex_restoration_record_the_outer_interval(self):
        cpu = item((ROOT / 'kernel/src/arch_impl/x86_64/cpu.rs').read_text(), '    fn without_interrupts<')
        mutex = (ROOT / 'kernel/src/irq_safe_mutex.rs').read_text().replace('use spin::', 'use crate::spin::')
        # Exercise the x86 branch even when this host is ARM.
        mutex = mutex.replace('#[cfg(target_arch = "aarch64")]', '#[cfg(any())]')
        mutex = mutex.replace('#[cfg(target_arch = "x86_64")]', '#[cfg(all())]')
        fixture = r'''
use std::cell::Cell;
thread_local!{static IRQS:Cell<bool>=const{Cell::new(true)};static CLOCK:Cell<u64>=const{Cell::new(1)};static MAX:Cell<u64>=const{Cell::new(0)};}
fn arch_interrupts_enabled()->bool{IRQS.with(Cell::get)}
unsafe fn arch_disable_interrupts(){IRQS.with(|i|i.set(false));}
unsafe fn arch_enable_interrupts(){IRQS.with(|i|i.set(true));}
fn advance(n:u64){CLOCK.with(|c|c.set(c.get()+n));}
mod tracing {pub mod providers {pub mod gpu {
 pub fn masked_begin(saved:u64)->u64{assert!(!crate::arch_interrupts_enabled());if saved&128!=0{0}else{crate::CLOCK.with(std::cell::Cell::get)}}
 pub fn masked_end(start:u64){assert!(!crate::arch_interrupts_enabled());if start!=0{let end=crate::CLOCK.with(std::cell::Cell::get);crate::MAX.with(|m|m.set(m.get().max(end-start)));}}
}}}
mod x86_64 {pub mod instructions {pub mod interrupts {
 pub fn without_interrupts<R>(f:impl FnOnce()->R)->R {
  let enabled=crate::arch_interrupts_enabled();unsafe{crate::arch_disable_interrupts();}
  let r=f();if enabled{unsafe{crate::arch_enable_interrupts();}}r
 }
}}}
mod spin {
 pub type MutexGuard<'a,T>=std::sync::MutexGuard<'a,T>;
 pub struct Mutex<T>(std::sync::Mutex<T>);
 impl<T> Mutex<T>{pub const fn new(v:T)->Self{Self(std::sync::Mutex::new(v))}
  pub fn lock(&self)->MutexGuard<'_,T>{self.0.lock().unwrap()}
  pub fn try_lock(&self)->Option<MutexGuard<'_,T>>{self.0.try_lock().ok()}}
}
mod irq_mutex { MUTEX }
struct Cpu;
impl Cpu {fn interrupts_enabled()->bool{arch_interrupts_enabled()} CPU}
#[test]fn both_masking_primitives_time_nested_intervals_without_resetting_the_maximum(){
 Cpu::without_interrupts(||{advance(3);Cpu::without_interrupts(||advance(7));advance(2);});
 assert!(arch_interrupts_enabled());assert_eq!(MAX.with(Cell::get),12);
 let m=irq_mutex::IrqSafeMutex::new(7);
 {let mut g=m.lock();*g=9;advance(501);assert!(m.try_lock().is_none());Cpu::without_interrupts(||advance(1));}
 assert!(arch_interrupts_enabled());assert_eq!(MAX.with(Cell::get),502);
 {let g=m.try_lock().unwrap();assert_eq!(*g,9);advance(1);}
 assert_eq!(MAX.with(Cell::get),502,"short intervals must not overwrite a bound violation");
 unsafe{arch_disable_interrupts();}
 Cpu::without_interrupts(||{let g=m.lock();assert_eq!(*g,9);advance(999);});
 assert!(!arch_interrupts_enabled());assert_eq!(MAX.with(Cell::get),502,"nested masks must not invent new intervals");
}
'''.replace('MUTEX', '\n'.join(line for line in mutex.splitlines() if not line.startswith('//!'))).replace('CPU', cpu)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'gauge.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                str(path / 'gauge.rs'), '-o', str(path / 'gauge')], check=True)
            subprocess.run([str(path / 'gauge')], check=True)


if __name__ == '__main__':
    unittest.main()
