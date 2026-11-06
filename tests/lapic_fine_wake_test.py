"""Execute the LAPIC wake driver against a controlled local register bank."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class LapicFineWake(unittest.TestCase):
    def test_one_shot_wakes_and_abandoned_request_preserve_the_regular_tick(self):
        driver = (ROOT / 'kernel/src/arch_impl/x86_64/apic/wakeup.rs').read_text().replace('//!', '//')
        fixture = r'''
use std::{cell::Cell,sync::atomic::{AtomicU32,AtomicBool,Ordering}};
thread_local! {static NOW:Cell<u64>=const{Cell::new(10_000_000)};static CPU:Cell<usize>=const{Cell::new(0)};static MASKED:Cell<bool>=const{Cell::new(false)};}
static TICKS:AtomicU32=AtomicU32::new(0);static FINE:AtomicU32=AtomicU32::new(0);static IPIS:AtomicU32=AtomicU32::new(0);
static NEED:AtomicBool=AtomicBool::new(false);
fn arch_without_interrupts<T>(f:impl FnOnce()->T)->T{let old=MASKED.with(|m|m.replace(true));let out=f();MASKED.with(|m|m.set(old));out}
mod per_cpu {pub fn cpu_id()->usize{crate::CPU.with(std::cell::Cell::get)}}
mod signal {pub fn monotonic_nanos()->u64{crate::NOW.with(std::cell::Cell::get)}}
mod time {
 pub mod timer {pub const MS_PER_TICK:u64=5;}
 pub mod wakeup {pub const POLL_NS:u64=250_000;pub fn record_wake(deadline:u64,now:u64){assert!(now>=deadline);crate::FINE.fetch_add(1,std::sync::atomic::Ordering::Relaxed);}}
}
mod task {pub mod scheduler {
 pub const MAX_CPUS:usize=2;
 pub fn set_need_resched(){crate::NEED.store(true,std::sync::atomic::Ordering::Relaxed);}
 pub fn is_need_resched()->bool{crate::NEED.load(std::sync::atomic::Ordering::Relaxed)}
 pub fn retry_after_interrupts_x86(){crate::IPIS.fetch_add(1,std::sync::atomic::Ordering::Relaxed);crate::NEED.store(false,std::sync::atomic::Ordering::Relaxed);}
}}
mod interrupts {pub mod timer {pub fn timer_interrupt_handler(_:u8,ipi:u8){crate::TICKS.fetch_add(u32::from(ipi==0),std::sync::atomic::Ordering::Relaxed);if ipi==0{crate::NEED.store(true,std::sync::atomic::Ordering::Relaxed);}}}}
mod arch_impl {pub mod x86_64 {
 pub mod msi {pub fn reserve(_:fn())->Result<u8,&'static str>{Ok(0x50)}}
 pub mod apic {
  use super::super::super::*;
  pub static TIMER_COUNT:AtomicU32=AtomicU32::new(5000);
  pub static LVT:AtomicU32=AtomicU32::new((1<<17)|32);
  pub static COUNT:AtomicU32=AtomicU32::new(5000);
  pub fn active()->bool{true}
  pub fn read(register:u32)->u32{assert!(MASKED.with(Cell::get));assert_eq!(register,0x390);3000}
  pub fn write(register:u32,value:u32){assert!(MASKED.with(Cell::get));match register {0x320=>{LVT.store(value,Ordering::Relaxed);},0x380=>{COUNT.store(value,Ordering::Relaxed);},_=>panic!("register {register:x}")}}
  pub fn start_local_timer(){LVT.store((1<<17)|32,Ordering::Relaxed);COUNT.store(5000,Ordering::Relaxed);}
  #[path="CLOCKEVENT"] mod clockevent;
  pub mod wakeup { DRIVER
   pub fn start(){init();}
   pub fn fire(){crate::arch_without_interrupts(interrupt);}
  }
 }
}}
fn main(){
 use arch_impl::x86_64::apic::{self,wakeup};
 wakeup::start();assert!(wakeup::request_wakeup(10_250_000));
 assert_eq!(apic::LVT.load(Ordering::Relaxed),0x50,"fine wake left the periodic vector selected");
 assert_eq!(apic::COUNT.load(Ordering::Relaxed),250,"fine wake used a 5ms timer reload");
 NOW.with(|n|n.set(10_250_000));wakeup::fire();assert_eq!(FINE.load(Ordering::Relaxed),1);assert_eq!(TICKS.load(Ordering::Relaxed),0);assert_eq!(IPIS.load(Ordering::Relaxed),1);
 assert_eq!(apic::COUNT.load(Ordering::Relaxed),2750); // No waiter cleanup is required.
 NOW.with(|n|n.set(13_000_000));wakeup::fire();assert_eq!(TICKS.load(Ordering::Relaxed),1);assert_eq!(apic::LVT.load(Ordering::Relaxed),(1<<17)|32);assert_eq!(apic::COUNT.load(Ordering::Relaxed),5000);
 assert_eq!(IPIS.load(Ordering::Relaxed),2,"regular tick lost its scheduling epilogue");
}
'''.replace('CLOCKEVENT', str(ROOT / 'kernel/src/arch_impl/x86_64/apic/clockevent.rs')).replace('DRIVER', driver)
        with tempfile.TemporaryDirectory(prefix='lapic-fine-') as directory:
            p = Path(directory)
            (p / 'check.rs').write_text(fixture)
            compiler = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '-Dwarnings']
            subprocess.run(compiler + [str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
