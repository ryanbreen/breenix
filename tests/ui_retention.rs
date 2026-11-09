//! Exercise the production upload controller with a deterministic process boundary.
extern crate self as libbreenix;
extern crate self as breenix_suite;
use std::cell::{Cell, RefCell};

#[path = "../libs/libuitest/src/retention.rs"]
pub mod retention;
#[path = "../libs/libuitest/src/artifacts.rs"]
pub mod artifacts;
#[path = "../libs/libuitest/src/arena.rs"]
pub mod arena;
pub mod frames {
    pub fn digest(bytes: &[u8]) -> String {
        libpixcheck::sha256(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }
}
#[derive(Clone, Copy)]
enum Mode { Success, Failure, Hung, Delayed(u64), WaitFailure }
thread_local! {
    static CLOCK: Cell<u64> = const { Cell::new(0) };
    static MODE: Cell<Mode> = const { Cell::new(Mode::Success) };
    static KILLED: Cell<bool> = const { Cell::new(false) };
    static REAPED: Cell<bool> = const { Cell::new(false) };
    static SENT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}
pub mod errno {
    #[derive(Debug)] pub enum Errno { EINTR }
}
pub mod error {
    #[derive(Debug)] pub enum Error { Os(super::errno::Errno), Other }
    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{self:?}") }
    }
}
pub struct Pid(u64);
impl Pid { pub fn raw(&self) -> u64 { self.0 } }
pub mod time {
    pub struct Stamp(u64);
    impl Stamp { pub fn as_nanos(&self) -> u128 { u128::from(self.0) * 1_000_000 } }
    pub fn now_monotonic() -> Result<Stamp, super::error::Error> { Ok(Stamp(super::CLOCK.get())) }
    pub fn sleep_ms(ms: u64) -> Result<(), super::error::Error> { super::CLOCK.set(super::CLOCK.get()+ms); Ok(()) }
}
pub mod signal {
    pub const SIGKILL: u32 = 9;
    pub fn kill(pid: i32, signal: u32) -> Result<(), super::error::Error> {
        assert_eq!((pid, signal), (123, SIGKILL));
        super::KILLED.set(true); Ok(())
    }
}
pub mod process {
    pub const WNOHANG: u32 = 1;
    pub fn waitpid(pid: i32, status: &mut i32, flags: u32) -> Result<super::Pid, super::error::Error> {
        assert_eq!(pid,123);
        if matches!(super::MODE.get(), super::Mode::WaitFailure) && flags == WNOHANG {
            return Err(super::error::Error::Other);
        }
        let complete = super::KILLED.get() || match super::MODE.get() {
            super::Mode::Hung => false,
            super::Mode::Delayed(at) => super::CLOCK.get() >= at,
            _ => true,
        };
        if !complete { assert_eq!(flags,WNOHANG); return Ok(super::Pid(0)); }
        assert!(!super::REAPED.replace(true), "child reaped twice");
        *status = if matches!(super::MODE.get(), super::Mode::Failure) {1} else {0};
        Ok(super::Pid(123))
    }
    pub fn wifexited(status: i32) -> bool { (0..=1).contains(&status) }
    pub fn wexitstatus(status: i32) -> i32 { status }
}
pub mod suite {
    pub enum Run<'a> { Call(&'a mut dyn FnMut() -> i32) }
    pub fn spawn(keep: &[(u64,u64)], Run::Call(send): Run<'_>) -> Result<super::Pid, super::error::Error> {
        assert!(keep.is_empty(), "upload inherited a runner descriptor");
        if !matches!(super::MODE.get(), super::Mode::Hung) { send(); }
        Ok(super::Pid(123))
    }
    pub fn upload_diagnostic(suite: &str, label: &str, bytes: &[u8]) -> Result<String,String> {
        assert_eq!(suite,"ui-testing"); assert_eq!(label,"uitest-0000000000000000");
        super::SENT.with_borrow_mut(|sent| *sent=bytes.to_vec());
        if matches!(super::MODE.get(),super::Mode::Failure) {Err("http-status".into())} else {Ok("receipt".into())}
    }
    pub fn case_ms_left() -> u64 {10000}
}
fn run(mode: Mode, budget: u64) -> Result<(), String> {
    CLOCK.set(0); MODE.set(mode); KILLED.set(false); REAPED.set(false); SENT.with_borrow_mut(Vec::clear);
    retention::transfer("ui-testing","uitest-0000000000000000",b"exact frozen snapshot",budget)
}
#[test]
fn accepted_and_failed_uploads_require_the_reaped_child_outcome() {
    assert_eq!(run(Mode::Success,250),Ok(()));
    SENT.with_borrow(|sent|assert_eq!(sent,b"exact frozen snapshot"));
    assert!(REAPED.get()); assert!(!KILLED.get());
    assert_eq!(run(Mode::Failure,250),Err("transfer".into()));
    assert!(REAPED.get()); assert!(!KILLED.get());
}
#[test]
fn blocked_sender_is_killed_and_reaped_at_one_absolute_deadline() {
    assert_eq!(run(Mode::Hung,250),Err("deadline".into()));
    assert_eq!(CLOCK.get(),250); assert!(KILLED.get()); assert!(REAPED.get());
    assert_eq!(run(Mode::WaitFailure,250),Err("wait".into()));
    assert!(KILLED.get()); assert!(REAPED.get());
    assert_eq!(run(Mode::Delayed(249),250),Ok(()));
    assert_eq!(CLOCK.get(),249); assert!(!KILLED.get());
    assert_eq!(run(Mode::Delayed(251),250),Err("deadline".into()));
    assert_eq!(CLOCK.get(),250); assert!(KILLED.get()); assert!(REAPED.get());
}
