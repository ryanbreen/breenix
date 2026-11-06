"""A refused fine arm preserves the production wait and its coarse timeout."""
from pathlib import Path
import os, subprocess, tempfile, sys
root=Path(__file__).resolve().parents[1]; rev=os.getenv('BREENIX_FINE_REV')
src=subprocess.check_output(['git','show',rev+':kernel/src/gpu/mod.rs'],text=True) if rev else (root/'kernel/src/gpu/mod.rs').read_text()
a=src.index('fn wait_event_prepared('); b=src.index('\n/// Send synchronous',a)
body=src[a:b]
fixture=r'''
use std::sync::atomic::{AtomicU64,Ordering};
static EVENTS:AtomicU64=AtomicU64::new(0);
struct Waiters;
impl Waiters {fn add_waiter(&self,_:u64)->bool{true} fn finish_wait_for(&self,_:u64){}}
static WAITERS:Waiters=Waiters;
enum PrepareOutcome {Queued}
mod time {pub mod wakeup {pub fn arm(_:u64)->bool{false}}}
mod task {pub mod scheduler {
 pub fn current_thread_id()->Option<u64>{Some(1)}
 pub struct Scheduler;impl Scheduler {pub fn block_current_for_io_with_timeout(&self,wake:Option<u64>)->bool{assert_eq!(wake,Some(250_000));true}}
 pub fn with_scheduler(f:impl FnOnce(&Scheduler)->bool)->Option<bool>{Some(f(&Scheduler))}
}}
mod syscall {
 pub mod errno {pub const ESRCH:i32=3;pub const ENOMEM:i32=12; OLD_ERRNO }
 pub mod blocking_io {pub fn wait_prepared(_: &crate::Waiters,_:crate::PrepareOutcome)->Result<(),i32>{Ok(())}}
}
BODY
fn main(){assert_eq!(wait_event_prepared(0,Some(250_000),true),Ok(()));}
'''.replace('BODY',body).replace('OLD_ERRNO','pub const ENODEV:i32=19;' if rev else '')
with tempfile.TemporaryDirectory() as tmp:
 p=Path(tmp)/'check.rs';p.write_text(fixture)
 command=['rustc','--edition=2021','-Dwarnings',str(p),'-o',str(p.with_suffix(''))]
 if sys.platform == 'darwin': command=['taskpolicy','-b']+command
 subprocess.run(command,check=True)
 subprocess.run([str(p.with_suffix(''))],check=True)
