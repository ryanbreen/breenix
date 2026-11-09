//! Exercise the production worker mailbox and its scheduler-lock query boundary.
extern crate self as spin;
use core::sync::atomic::{AtomicBool,AtomicU32,Ordering};
static QUERY_ONLY:AtomicBool=AtomicBool::new(false);
pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    pub const fn new(value:T)->Self { Self(std::sync::Mutex::new(value)) }
    pub fn lock(&self)->std::sync::MutexGuard<'_,T> {
        assert!(!QUERY_ONLY.load(Ordering::Relaxed),"pending query acquired a mailbox lock under the scheduler lock");
        self.0.lock().unwrap()
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
fn request(flag:&AtomicBool) {
    SLICES.store(0,Ordering::Relaxed);
    flag.store(true,Ordering::Release);
    if MODE.load(Ordering::Relaxed)>=2 { assert!(suite_replay::take().is_some()); }
    if EARLY.load(Ordering::Relaxed) { run_worker(); }
}
pub mod task { pub mod scheduler {
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
}

fn start_killed(mode:u32) {
    test_control::ENABLED.store(true,Ordering::Relaxed);
    MODE.store(mode,Ordering::Relaxed);
    FATAL.store(mode!=3,Ordering::Release);
}
fn next_submit() {
    FATAL.store(false,Ordering::Release);
    MODE.store(0,Ordering::Relaxed);
    let receipt=suite_replay::submit(suite_replay::Request::Keyboard,false).unwrap();
    assert_eq!((receipt.calls,receipt.posted,receipt.ring),(2,4,4));
}
#[test]
fn sigkill_before_take_cancels_request() {
    start_killed(1);
    assert_eq!(suite_replay::submit(suite_replay::Request::Keyboard,false).unwrap_err(),4);
    assert!(!suite_replay::pending());
    next_submit();
}
#[test]
fn sigkill_after_take_keeps_worker_custody_until_answer() {
    start_killed(2);
    assert_eq!(suite_replay::submit(suite_replay::Request::Keyboard,false).unwrap_err(),4);
    assert!(!suite_replay::pending());
    assert_eq!(suite_replay::submit(suite_replay::Request::Keyboard,false).unwrap_err(),16);
    suite_replay::answer(Ok(input::control::Receipt{calls:9,posted:9,ring:9}));
    next_submit();
}
#[test]
fn sigkill_racing_answer_releases_mailbox_once() {
    start_killed(3);
    match suite_replay::submit(suite_replay::Request::Keyboard,false) {
        Err(e) => assert_eq!(e,4),
        Ok(receipt) => assert_eq!((receipt.calls,receipt.posted,receipt.ring),(9,9,9)),
    }
    assert!(!suite_replay::pending());
    next_submit();
}
