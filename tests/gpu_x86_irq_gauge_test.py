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
