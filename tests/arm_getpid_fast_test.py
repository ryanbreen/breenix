"""Execute ARM getpid while the process-manager lookup is unavailable."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class ArmGetpid(unittest.TestCase):
    def test_threads_read_the_shared_immutable_group_without_manager_lookup(self):
        arm=item((ROOT/'kernel/src/arch_impl/aarch64/syscall_entry.rs').read_text(),'fn sys_getpid(')
        gpu=item((ROOT/'kernel/src/gpu/mod.rs').read_text(),'pub(crate) fn caller(').replace('pub(crate)', 'pub')
        shared=item((ROOT/'kernel/src/syscall/handlers.rs').read_text(),'pub fn sys_getpid(')
        shared=shared[:shared.index('    // Disable interrupts when accessing process manager')]+'''    panic!("caller has no immutable group");
}'''
        run_rust(self,r'''
use std::sync::atomic::{AtomicU64,Ordering};
static TID:AtomicU64=AtomicU64::new(7);
pub fn arch_without_interrupts<T>(f:impl FnOnce()->T)->T{f()}
pub mod task {
 pub mod thread{pub struct Account{pub process_id:u64}pub struct Thread{pub cpu_account:Option<Account>}}
 pub mod scheduler{pub fn current_thread_id()->Option<u64>{Some(crate::TID.load(crate::Ordering::Relaxed))}}
}
static THREAD:task::thread::Thread=task::thread::Thread{cpu_account:Some(task::thread::Account{process_id:42})};
pub mod per_cpu_aarch64{pub fn current_thread_ptr()->*const u8{(&crate::THREAD as *const crate::task::thread::Thread).cast()}}
pub mod arch_impl {
 pub trait PerCpuOps{fn current_thread_ptr()->*const u8;}
 pub mod x86_64{pub mod percpu{pub struct X86PerCpu;impl crate::arch_impl::PerCpuOps for X86PerCpu{
 fn current_thread_ptr()->*const u8{crate::per_cpu_aarch64::current_thread_ptr()}
 }}}
}
pub mod process {
 pub struct Pid;impl Pid{pub fn as_u64(&self)->u64{42}}
 pub struct Process{pub thread_group_id:Option<u64>}
 pub struct Manager;impl Manager{pub fn find_process_by_thread(&self,_:u64)->Option<(Pid,Process)>{None}}
 pub fn with_process_manager<R>(_:impl FnOnce(&Manager)->R)->Option<R>{panic!("GPU caller waited for the process manager")}
 pub fn manager()->&'static Option<Manager>{panic!("getpid waited for the process manager")}
}
mod gpu {
 const EINVAL:i64=22;
 @GPU@
}
pub mod syscall {
 #[derive(Debug)]pub enum SyscallResult{Ok(u64),Err(u64)}
 pub mod handlers{use super::SyscallResult;@SHARED@}
}
fn result_to_u64(result:syscall::SyscallResult)->u64{match result{syscall::SyscallResult::Ok(n)=>n,syscall::SyscallResult::Err(e)=> (-(e as i64))as u64}}
@ARM@
#[test]fn same_process_from_each_thread_without_manager(){
 let _=result_to_u64;
 for tid in [7,8]{TID.store(tid,Ordering::Relaxed);assert_eq!(sys_getpid(),42);assert_eq!(gpu::caller(),Ok(42));}
}
'''.replace('@SHARED@',shared).replace('@ARM@',arm).replace('@GPU@',gpu))
if __name__=='__main__':unittest.main()
