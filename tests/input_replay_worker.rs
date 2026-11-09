//! Exercise the production worker mailbox and its scheduler-lock query boundary.
extern crate self as spin;
use core::sync::atomic::{AtomicBool,AtomicU32,AtomicU64,Ordering};
static QUERY_ONLY:AtomicBool=AtomicBool::new(false);
pub struct Mutex<T>(std::sync::Mutex<T>);
pub struct Guard<'a,T>(std::sync::MutexGuard<'a,T>);
static MAILBOX_LOCKS:AtomicU32=AtomicU32::new(0);
impl<T> core::ops::Deref for Guard<'_,T> {
    type Target=T;
    fn deref(&self)->&T { &self.0 }
}
impl<T> core::ops::DerefMut for Guard<'_,T> {
    fn deref_mut(&mut self)->&mut T { &mut self.0 }
}
impl<T> Drop for Guard<'_,T> {
    fn drop(&mut self) { MAILBOX_LOCKS.fetch_sub(1,Ordering::Relaxed); }
}
impl<T> Mutex<T> {
    pub const fn new(value:T)->Self { Self(std::sync::Mutex::new(value)) }
    pub fn lock(&self)->Guard<'_,T> {
        assert!(!QUERY_ONLY.load(Ordering::Relaxed),"pending query acquired a mailbox lock under the scheduler lock");
        let guard=self.0.lock().unwrap();
        MAILBOX_LOCKS.fetch_add(1,Ordering::Relaxed);
        Guard(guard)
    }
}
#[path = "../kernel/src/drivers/xhci/suite_replay.rs"] mod suite_replay;
mod input { pub mod control {
    #[derive(Clone,Copy,Debug)] pub struct Receipt { pub calls:u32,pub posted:u32,pub ring:u32 }
} }
mod syscall {
    pub mod errno { pub const ENOSYS:i32=38;pub const EBUSY:i32=16;pub const EIO:i32=5;pub const EINTR:i32=4; }
    pub fn fatal_signal_pending()->bool { crate::FATAL.load(crate::Ordering::Acquire) }
}
mod test_control {
    pub static ENABLED:super::AtomicBool=super::AtomicBool::new(false);
    pub fn enabled()->bool { ENABLED.load(super::Ordering::Relaxed) }
}
static FAILED:AtomicBool=AtomicBool::new(false);
static LEDS:AtomicBool=AtomicBool::new(false);
static IRQ_ENABLED:AtomicBool=AtomicBool::new(false);
static EARLY:AtomicBool=AtomicBool::new(false);
static WAITS:AtomicU32=AtomicU32::new(0);
static SLICES:AtomicU32=AtomicU32::new(0);
static FATAL:AtomicBool=AtomicBool::new(false);
// 0 answers normally, 1 leaves the request pending, 2 takes it, 3 races an answer.
static MODE:AtomicU32=AtomicU32::new(0);
static WORKER_TID:AtomicU64=AtomicU64::new(73);
static NOW:AtomicU64=AtomicU64::new(123_000);
static SNAPSHOT_BUSY:AtomicBool=AtomicBool::new(false);
struct Work;
impl Work {
    fn pending(&self)->bool { true }
    fn waiter(&self)->u64 { 73 }
}
static WORK:Work=Work;
fn now()->u64 { NOW.load(Ordering::Relaxed) }
fn interrupt_count()->u64 { 17 }
pub mod log_notice {
    pub static LINES:std::sync::Mutex<Vec<String>>=std::sync::Mutex::new(Vec::new());
    pub struct Record;
    impl Record {
        pub const fn new()->Self { Self }
        pub fn post_again(&'static self,args:core::fmt::Arguments)->bool {
            // The diagnostic must not retain either mailbox lock while posting.
            assert_eq!(crate::MAILBOX_LOCKS.load(crate::Ordering::Relaxed),0);
            LINES.try_lock().expect("diagnostic sink must not block").push(format!("{args}\n"));
            true
        }
    }
}
fn request(flag:&AtomicBool) {
    SLICES.store(0,Ordering::Relaxed);
    flag.store(true,Ordering::Release);
    NOW.fetch_add(10_000,Ordering::Relaxed);
    if MODE.load(Ordering::Relaxed)>=2 { assert!(suite_replay::take().is_some()); }
    if EARLY.load(Ordering::Relaxed) { run_worker(); }
}
pub mod task { pub mod thread {
    #[derive(Clone,Copy,Debug)] pub enum ThreadState { BlockedOnIO }
} pub mod scheduler {
    pub fn try_thread_wait(tid:u64)->Option<(super::thread::ThreadState,Option<usize>,Option<u64>)> {
        assert_eq!(tid,73);
        (!crate::SNAPSHOT_BUSY.load(crate::Ordering::Relaxed))
            .then_some((super::thread::ThreadState::BlockedOnIO,Some(3),Some(99_000_000)))
    }
    pub fn yield_current() {
        assert!(crate::IRQ_ENABLED.load(crate::Ordering::Relaxed),
            "a reschedule flag cannot deliver MSI while the caller masks interrupts");
        crate::run_worker();
    }
} pub mod completion {
    pub struct Completion(crate::AtomicBool);
    impl Completion {
        pub const fn new()->Self { Self(crate::AtomicBool::new(false)) }
        pub fn reset(&self) { self.0.store(false,crate::Ordering::Relaxed); }
        pub fn complete(&self,token:u32) { assert_eq!(token,1);self.0.store(true,crate::Ordering::Relaxed); }
        pub fn wait_timeout_uninterruptible(&self,token:u32,ns:u64)->Result<bool,i32> {
            assert_eq!(token,1);assert_eq!(ns,1_000_000_000);
            crate::WAITS.fetch_add(1,crate::Ordering::Relaxed);
            assert!(crate::SLICES.fetch_add(1,crate::Ordering::Relaxed)<4,"submit ignored a pending SIGKILL");
            if crate::MODE.load(crate::Ordering::Relaxed)!=0 { return Ok(false); }
            if self.0.load(crate::Ordering::Relaxed) { return Ok(true); }
            // The production Completion releases syscall preemption and enters
            // an IRQ-enabled blocked wait; only then can the device finish.
            crate::IRQ_ENABLED.store(true,crate::Ordering::Relaxed);
            crate::run_worker();
            crate::IRQ_ENABLED.store(false,crate::Ordering::Relaxed);
            assert!(self.0.load(crate::Ordering::Relaxed));
            Ok(true)
        }
        pub fn wait_timeout_killable(&self,token:u32,ns:u64)->Result<bool,i32> {
            assert_eq!(token,1);assert_eq!(ns,1_000_000_000);
            crate::WAITS.fetch_add(1,crate::Ordering::Relaxed);
            assert!(crate::SLICES.fetch_add(1,crate::Ordering::Relaxed)<4,"submit ignored a pending SIGKILL");
            if crate::MODE.load(crate::Ordering::Relaxed)==3 {
                crate::FATAL.store(true,crate::Ordering::Release);
                crate::suite_replay::answer(Ok(crate::input::control::Receipt{calls:9,posted:9,ring:9}));
            }
            if crate::syscall::fatal_signal_pending() { return Err(crate::syscall::errno::EINTR); }
            if self.0.load(crate::Ordering::Relaxed) { return Ok(true); }
            crate::IRQ_ENABLED.store(true,crate::Ordering::Relaxed);
            crate::run_worker();
            crate::IRQ_ENABLED.store(false,crate::Ordering::Relaxed);
            Ok(self.0.load(crate::Ordering::Relaxed))
        }
    }
} }
fn run_worker() {
        use crate::{suite_replay, input::control::Receipt, Ordering};
        assert!(crate::LEDS.swap(false,Ordering::AcqRel));
        assert!(suite_replay::pending());
        assert_eq!(suite_replay::submit(suite_replay::Request::Keyboard,false).unwrap_err(),16);
        let (request,suppress)=suite_replay::take().unwrap();
        assert!(!suite_replay::pending());
        if let suite_replay::Request::Pointer(x,y)=request { assert_eq!((x,y),(23,42)); }
        suite_replay::answer(Ok(Receipt{calls:if suppress {0} else {2},posted:4,ring:4}));
}
#[test]
fn mailbox_keeps_scheduler_queries_lock_free_and_serializes_transactions() {
    use suite_replay::{Request,submit,pending};
    assert_eq!(submit(Request::Keyboard,false).unwrap_err(),38);
    QUERY_ONLY.store(true,Ordering::Relaxed);
    assert!(!pending());
    QUERY_ONLY.store(false,Ordering::Relaxed);
    test_control::ENABLED.store(true,Ordering::Relaxed);
    for (request,suppress,early) in [(Request::Keyboard,false,false),(Request::Pointer(23,42),true,false),(Request::Keyboard,false,true)] {
        EARLY.store(early,Ordering::Relaxed);
        let waits=WAITS.load(Ordering::Relaxed);
        let receipt=submit(request,suppress).unwrap();
        assert!(WAITS.load(Ordering::Relaxed)>waits,"must observe completion before reusing its token, even for an early answer");
        assert_eq!((receipt.calls,receipt.posted,receipt.ring),(if suppress {0} else {2},4,4));
        assert!(!pending());
    }
    assert!(log_notice::LINES.lock().unwrap().is_empty(),"successful replay emitted a failure diagnostic");
}

fn start_killed(mode:u32) {
    test_control::ENABLED.store(true,Ordering::Relaxed);
    MODE.store(mode,Ordering::Relaxed);
    FATAL.store(mode!=3,Ordering::Release);
    NOW.store(123_000,Ordering::Relaxed);
    assert_eq!((WORKER_TID.load(Ordering::Relaxed),WORK.pending(),WORK.waiter(),interrupt_count()),(73,true,73,17));
    assert_eq!(now(),123_000);
}
fn next_submit() {
    FATAL.store(false,Ordering::Release);
    MODE.store(0,Ordering::Relaxed);
    let receipt=suite_replay::submit(suite_replay::Request::Keyboard,false).unwrap();
    assert_eq!((receipt.calls,receipt.posted,receipt.ring),(2,4,4));
    assert!(log_notice::LINES.lock().unwrap().is_empty(),"successful reuse emitted a failure diagnostic");
}
fn assert_diag(state:&str) {
    let lines=std::mem::take(&mut *log_notice::LINES.lock().unwrap());
    assert_eq!(lines,vec![format!("XHCI_REPLAY_STATE state={state} age_ns=10000 worker_tid=73 worker_state=Some(BlockedOnIO) cpu=Some(3) wake_time_ns=Some(99000000) work_pending=true waiter=73 leds=true interrupts=17\n")]);
}
#[test]
fn sigkill_before_take_cancels_request() {
    start_killed(1);
    assert_eq!(suite_replay::submit(suite_replay::Request::Keyboard,false).unwrap_err(),4);
    assert!(!suite_replay::pending());
    assert_diag("Posted");
    next_submit();
}
#[test]
fn sigkill_after_take_keeps_worker_custody_until_answer() {
    start_killed(2);
    assert_eq!(suite_replay::submit(suite_replay::Request::Keyboard,false).unwrap_err(),4);
    assert!(!suite_replay::pending());
    assert_diag("Taken");
    assert_eq!(suite_replay::submit(suite_replay::Request::Keyboard,false).unwrap_err(),16);
    suite_replay::answer(Ok(input::control::Receipt{calls:9,posted:9,ring:9}));
    next_submit();
}
#[test]
fn sigkill_racing_answer_releases_mailbox_once() {
    start_killed(3);
    match suite_replay::submit(suite_replay::Request::Keyboard,false) {
        Err(e) => { assert_eq!(e,4); assert_diag("Taken"); }
        Ok(receipt) => {
            assert_eq!((receipt.calls,receipt.posted,receipt.ring),(9,9,9));
            assert!(log_notice::LINES.lock().unwrap().is_empty(),"answer won the race, so this is success");
        }
    }
    assert!(!suite_replay::pending());
    next_submit();
}

#[test]
fn sigkill_diagnostic_does_not_wait_for_worker_snapshot() {
    start_killed(1);
    SNAPSHOT_BUSY.store(true,Ordering::Relaxed);
    assert_eq!(suite_replay::submit(suite_replay::Request::Keyboard,false).unwrap_err(),4);
    let lines=std::mem::take(&mut *log_notice::LINES.lock().unwrap());
    assert_eq!(lines,vec!["XHCI_REPLAY_STATE state=Posted age_ns=10000 worker_tid=73 worker_state=None cpu=None wake_time_ns=None work_pending=true waiter=73 leds=true interrupts=17\n"]);
    next_submit();
}
