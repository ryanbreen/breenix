"""Execute the ARM production fine-deadline expiry with overlapping waits."""
from pathlib import Path
import os, subprocess
root=Path(__file__).resolve().parents[1]
rev=os.getenv('BREENIX_FINE_REV')
src=subprocess.check_output(['git','show',rev+':kernel/src/arch_impl/aarch64/timer_interrupt.rs'],text=True) if rev else (root/'kernel/src/arch_impl/aarch64/timer_interrupt.rs').read_text()
a=src.index('fn fine_interval('); b=src.index('\n/// Arm the timer',a)
body=src[a:b]
a=src.index('static FINE_DEADLINES:'); b=src.index('/// Check if the timer',a)
static=src[a:b]
if 'struct FineDeadlines' in src: static=src[src.index('struct FineDeadlines'):b]
init='unsafe { let d=&mut *FINE_DEADLINES[0].0.get(); assert!(d.insert(250_000)); assert!(d.insert(1_000_000_000)); }' if 'FineDeadlines' in static else 'FINE_DEADLINES[0].store(250_000,Ordering::Relaxed); FINE_LAST[0].store(1_000_000_000,Ordering::Relaxed);'
fixture=r'''use std::sync::atomic::{AtomicU64,Ordering};
static NOW:AtomicU64=AtomicU64::new(250_000);
fn arch_without_interrupts<T>(f:impl FnOnce()->T)->T{f()}
mod signal {pub fn monotonic_nanos()->u64 {crate::NOW.load(std::sync::atomic::Ordering::Relaxed)}}
mod time {#[path="QUEUE"] pub mod deadlines; pub mod wakeup {pub const POLL_NS:u64=250_000; pub fn record_wake(_:u64,_:u64){}}}
mod arch {
 pub mod constants {pub const MAX_CPUS:usize=1;}
 pub mod timer {pub fn frequency_hz()->u64 {1_000_000_000}}
 pub mod percpu {pub struct Aarch64PerCpu; impl Aarch64PerCpu {pub fn cpu_id()->u64{0}}}
 pub mod driver {use super::super::*; STATIC BODY
 pub fn check(){ INIT assert_eq!(fine_interval(5_000_000,true),5_000_000); NOW.store(5_000_000,Ordering::Relaxed); assert_eq!(fine_interval(5_000_000,true),5_000_000);}}
}
fn main(){arch::driver::check();}
'''
fixture=fixture.replace('QUEUE',str(root/'kernel/src/time/deadlines.rs')).replace('STATIC',static).replace('BODY',body).replace('INIT',init)
if 'FineDeadlines' in static: fixture=fixture.replace('pub const POLL_NS:u64=250_000;','')
else: fixture=fixture.replace('#[path="'+str(root/'kernel/src/time/deadlines.rs')+'"] pub mod deadlines;','')
import tempfile, sys
with tempfile.TemporaryDirectory(prefix='arm-fine-') as tmp:
    p=Path(tmp)/'check.rs'; p.write_text(fixture)
    command=['rustc','--edition=2021','-Dwarnings',str(p),'-o',str(p.with_suffix(''))]
    if sys.platform == 'darwin': command=['taskpolicy','-b']+command
    subprocess.run(command,check=True)
    subprocess.run([str(p.with_suffix(''))],check=True)
