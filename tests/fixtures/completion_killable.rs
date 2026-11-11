// Execute the production completion and signal state on a host thread.
// Only scheduling, IRQ masking, slab storage and unrelated timers are fakes.
#![feature(allocator_api)]
#![allow(dead_code)] // Production modules expose APIs outside this scoped check.
extern crate alloc;
use std::cell::RefCell;

mod memory {
    pub mod slab {
        pub struct Slab;
        impl Slab { pub fn alloc(&self) -> Option<*mut u8> { None } }
        pub static SIGNAL_HANDLERS_SLAB: Slab = Slab;
        pub struct SlabBox<T>(Box<T>);
        impl<T> SlabBox<T> {
            pub fn from_box(value: Box<T>) -> Self { Self(value) }
            pub unsafe fn from_slab(_: *mut T, _: &Slab) -> Self { unreachable!() }
        }
        impl<T> std::ops::Deref for SlabBox<T> {
            type Target = T;
            fn deref(&self) -> &T { &self.0 }
        }
        impl<T> std::ops::DerefMut for SlabBox<T> {
            fn deref_mut(&mut self) -> &mut T { &mut self.0 }
        }
        impl Clone for SlabBox<crate::signal::types::SignalTable> {
            fn clone(&self) -> Self {
                // SignalTable contains only two arrays of Copy ABI records.
                Self(Box::new(unsafe { std::ptr::read(&**self) }))
            }
        }
    }
}
extern crate self as spin;
#[derive(Default)] pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self { Self(std::sync::Mutex::new(value)) }
    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
    pub fn try_lock(&self) -> Option<std::sync::MutexGuard<'_, T>> { self.0.try_lock().ok() }
}
mod ipc { pub mod fd { pub struct FdTableRef; } }
mod time {
    pub fn get_monotonic_time_ns() -> (u64,u64) {
        let now=crate::NOW.fetch_add(1_000_000, std::sync::atomic::Ordering::Relaxed);
        (now/1_000_000_000, now%1_000_000_000)
    }
}
mod signal {
    #[path = "@ROOT@/kernel/src/signal/constants.rs"] pub mod constants;
    #[path = "@ROOT@/kernel/src/signal/timer_signal.rs"] mod timer_signal;
    pub mod timers {
        pub use super::timer_signal::TimerSignal;
        pub const SI_TIMER: i32 = -2;
        #[derive(Default)] pub struct PosixTimers;
        impl PosixTimers {
            pub fn take_next_due(&self, _: &super::types::ThreadSignals)
                -> Option<(u32, super::types::SigInfo, bool, std::sync::Arc<TimerSignal>)> { None }
            pub fn try_disarm_all(&self) {}
            pub fn is_active(&self) -> bool { false }
        }
    }
    #[path = "@ROOT@/kernel/src/signal/types.rs"] pub mod types;
    pub use types::ThreadSignals;
}

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
static PREEMPT:AtomicU32=AtomicU32::new(1);
static NOW:AtomicU64=AtomicU64::new(0);
static BLOCKS:AtomicU32=AtomicU32::new(0);
static STAGE:AtomicU32=AtomicU32::new(0);
static IRQ_MASKED:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
thread_local! {
    static SIGNALS:RefCell<signal::types::SignalState>=RefCell::new(signal::types::SignalState::new());
    static CURRENT:RefCell<task::scheduler::Scheduler>=RefCell::new(task::scheduler::Scheduler::new());
}
fn send_kill() {
    SIGNALS.with(|state| {
        let state=state.borrow();
        assert!(state.thread.post_sent(signal::constants::SIGKILL,42,0,true));
        assert_eq!(state.thread.mirror.pending(),0,"kill must use the uncollected sender path");
    });
}
pub fn arch_without_interrupts<R>(f:impl FnOnce()->R)->R {
    let masked=IRQ_MASKED.swap(true,Ordering::Relaxed);
    let result=f();
    IRQ_MASKED.store(masked,Ordering::Relaxed);
    result
}
mod per_cpu {
    pub fn preempt_count()->u32 { crate::PREEMPT.load(crate::Ordering::Relaxed) }
    pub fn preempt_disable() { crate::PREEMPT.fetch_add(1,crate::Ordering::Relaxed); }
    pub fn preempt_enable() {
        assert_eq!(crate::PREEMPT.fetch_sub(1,crate::Ordering::Relaxed),1);
        if crate::STAGE.load(crate::Ordering::Relaxed)==2 { crate::send_kill(); }
    }
    pub fn cpu_id()->usize { 0 }
    pub fn current_thread()->Option<crate::task::thread::Thread> {
        assert!(crate::IRQ_MASKED.load(crate::Ordering::Relaxed));
        Some(crate::task::thread::Thread::new())
    }
}
extern crate self as x86_64;
pub mod instructions { pub mod interrupts {
    pub fn without_interrupts<R>(f:impl FnOnce()->R)->R { crate::arch_without_interrupts(f) }
    pub fn disable() { crate::IRQ_MASKED.store(true,crate::Ordering::Relaxed); }
    pub fn enable_and_hlt() { panic!("syscall wait must not use idle's halt"); }
} }
mod task {
    #[path = "@ROOT@/kernel/src/task/completion.rs"] pub mod completion;
    pub mod thread {
        pub struct WaitSite;
        impl WaitSite { pub fn enter(_: u64, _: u64) -> Self { Self } }

        #[derive(Clone,Copy,Debug,PartialEq)] pub enum ThreadState { Running, BlockedOnIO }
        pub struct Thread {
            pub state:ThreadState,
            pub blocked_in_syscall:bool,
            pub uninterruptible_io:bool,
            pub wake_time_ns:Option<u64>,
            pub signals:std::sync::Arc<crate::signal::types::ThreadSignals>,
        }
        impl Thread {
            pub fn new()->Self {
                Self { state:ThreadState::Running,blocked_in_syscall:false,uninterruptible_io:false,
                    wake_time_ns:None,signals:crate::SIGNALS.with(|s|s.borrow().thread.clone()) }
            }
            pub fn set_running(&mut self) { self.state=ThreadState::Running; }
            pub fn needs_user_return_check(&self)->bool { self.signals.needs_return_check() }
        }
    }
    pub mod scheduler {
        pub struct Scheduler { pub thread:super::thread::Thread }
        impl Scheduler {
            pub fn new()->Self { Self { thread:super::thread::Thread::new() } }
            pub fn current_thread_mut(&mut self)->Option<&mut super::thread::Thread> { Some(&mut self.thread) }
            pub fn idle_thread(&self)->u64 { 0 }
            pub fn block_current_for_io_with_timeout(&mut self,deadline:Option<u64>) {
                assert_eq!(crate::PREEMPT.load(crate::Ordering::Relaxed),0);
                self.thread.state=super::thread::ThreadState::BlockedOnIO;
                self.thread.blocked_in_syscall=true;
                self.thread.wake_time_ns=deadline;
                crate::BLOCKS.fetch_add(1,crate::Ordering::Relaxed);
            }
        }
        pub fn current_thread_id()->Option<u64> { Some(1) }
        pub fn with_current<R>(f:impl FnOnce(&mut Scheduler)->R)->Option<R> {
            crate::CURRENT.with(|s|Some(f(&mut s.borrow_mut())))
        }
        pub fn isr_unblock_for_io(tid:u64) { assert_eq!(tid,1); }
        pub fn kick_cpu_x86(cpu:usize) { assert_eq!(cpu,0); }
    }
    pub mod waitqueue {
        pub fn halt_blocked_current() {
            crate::CURRENT.with(|s| {
                let s=s.borrow();
                assert_eq!(s.thread.state,super::thread::ThreadState::BlockedOnIO);
                assert!(!s.thread.uninterruptible_io,"killable wait hid itself from the sender wake");
            });
            if crate::STAGE.load(crate::Ordering::Relaxed)==3 { crate::send_kill(); }
            else { crate::COMPLETION.complete(1); }
        }
    }
}
mod syscall {
    pub mod errno { pub const EINTR:i32=4; }
    @CHECK_SIGNALS@
    @FATAL_SIGNAL@
    @WAIT_SIGNALS@
}
static COMPLETION:task::completion::Completion=task::completion::Completion::new();
fn check_exit(expected:Result<bool,i32>,blocks:u32) {
    assert_eq!(COMPLETION.wait_timeout_killable(1,1_000_000_000),expected);
    assert_eq!(COMPLETION.waiter_tid(),0,"wait left its registration behind");
    assert_eq!(PREEMPT.load(Ordering::Relaxed),1,"syscall preemption was not restored");
    assert_eq!(BLOCKS.load(Ordering::Relaxed),blocks);
    CURRENT.with(|s| {
        let s=s.borrow();
        assert_eq!(s.thread.state,task::thread::ThreadState::Running);
        assert!(!s.thread.blocked_in_syscall);
        assert!(!s.thread.uninterruptible_io);
    });
    // Reuse the very same completion after retiring the signal.
    SIGNALS.with(|s| *s.borrow_mut()=signal::types::SignalState::new());
    COMPLETION.reset();
    STAGE.store(0,Ordering::Relaxed);
    COMPLETION.complete(2);
    assert_eq!(COMPLETION.wait_timeout_killable(2,1_000_000_000),Ok(true));
}
#[test]
fn row_pending_sigkill_interrupts_before_sleep() {
    SIGNALS.with(|s|s.borrow_mut().set_pending(signal::constants::SIGKILL));
    check_exit(Err(4),0);
}
#[test]
fn uncollected_sigkill_interrupts_before_sleep() {
    send_kill();
    check_exit(Err(4),0);
}
#[test]
fn sigkill_between_preemption_release_and_block_interrupts() {
    STAGE.store(2,Ordering::Relaxed);
    check_exit(Err(4),0);
}
#[test]
fn sigkill_after_block_interrupts_and_retires_waiter() {
    STAGE.store(3,Ordering::Relaxed);
    check_exit(Err(4),1);
}
#[test]
fn sigterm_does_not_interrupt_a_killable_completion() {
    SIGNALS.with(|s|s.borrow_mut().set_pending(signal::constants::SIGTERM));
    check_exit(Ok(true),1);
}
