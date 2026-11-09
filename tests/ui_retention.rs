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
    static EXIT: Cell<i32> = const { Cell::new(0) };
    static REPORTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
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
pub mod memory {
    use std::sync::atomic::AtomicU64;
    pub const PROT_READ:i32=1; pub const PROT_WRITE:i32=2;
    pub const MAP_SHARED:i32=1; pub const MAP_ANONYMOUS:i32=32;
    pub fn mmap(_: *mut u8, length:usize, prot:i32, flags:i32, fd:i32, offset:i64)
        -> Result<*mut u8, super::error::Error> {
        assert_eq!((length,prot,flags,fd,offset),(4096,3,33,-1,0));
        Ok(Box::into_raw(Box::new([const {AtomicU64::new(0)};3])).cast())
    }
    pub fn munmap(ptr:*mut u8,length:usize)->Result<(),super::error::Error> {
        assert_eq!(length,4096);
        unsafe {drop(Box::from_raw(ptr.cast::<[AtomicU64;3]>()));} Ok(())
    }
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
        *status = super::EXIT.get();
        Ok(super::Pid(123))
    }
    pub fn wifexited(status: i32) -> bool { (0..=1).contains(&status) }
    pub fn wexitstatus(status: i32) -> i32 { status }
}
pub mod suite {
    pub mod control {
        pub const HANG_MS:u64=10_000;
        pub enum Wait{DiagnosticUpload}
        pub struct Watchdog(u64);
        impl Watchdog {
            pub fn new(_:Wait)->Result<Self,String>{Ok(Self(crate::CLOCK.get()+HANG_MS))}
            pub fn deadline_ms(&self)->i128{i128::from(self.0)}
        }
    }
    pub enum Run<'a> { Call(&'a mut dyn FnMut() -> i32) }
    pub fn spawn(keep: &[(u64,u64)], Run::Call(send): Run<'_>) -> Result<super::Pid, super::error::Error> {
        assert!(keep.is_empty(), "upload inherited a runner descriptor");
        if !matches!(super::MODE.get(), super::Mode::Hung) { super::EXIT.set(send()); }
        Ok(super::Pid(123))
    }
    pub fn upload_diagnostic(suite: &str, label: &str, bytes: &[u8]) -> Result<String,String> {
        assert_eq!(suite,"ui-testing"); assert_eq!(label,"uitest-0000000000000000");
        super::SENT.with_borrow_mut(|sent| *sent=bytes.to_vec());
        if matches!(super::MODE.get(),super::Mode::Failure) {Err("http-status".into())} else {Ok("receipt".into())}
    }
    pub fn case_ms_left() -> u64 {10000}
    pub fn value(name:&str,value:i64,unit:&str,_:Option<(i64,i64)>) {
        super::REPORTS.with_borrow_mut(|records|records.push(format!("{name}={value}{unit}")));
    }
    pub fn evidence_bundle(label: &str, bytes: usize, hash: &str, result: &str, path: &str) -> Result<(), String> {
        super::REPORTS.with_borrow_mut(|records| records.push(format!("{label} bytes={bytes} sha256={hash} result={result} path={path}")));
        Ok(())
    }
}
fn run(mode: Mode, budget: u64) -> Result<(), String> {
    CLOCK.set(0); EXIT.set(0); MODE.set(mode); KILLED.set(false); REAPED.set(false); SENT.with_borrow_mut(Vec::clear);
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

#[test]
fn retention_uses_the_runner_record_transport_even_with_silenced_stdout() {
    REPORTS.with_borrow_mut(Vec::clear);
    let _ = retention::retain("ui-testing", "diagnosis/transport-check");
    REPORTS.with_borrow(|records| {
        assert_eq!(records.len(), 1, "retention bypassed the case record transport");
        assert!(records[0].contains("result=disk-only:"));
        assert!(records[0].ends_with("path=/var/uitest/ui-testing/diagnosis/transport-check"));
    });
}

#[test]
fn failure_stamps_distinguish_sender_entry_from_exit_cleanup() {
    REPORTS.with_borrow_mut(Vec::clear);
    assert_eq!(run(Mode::Hung,250),Err("deadline".into()));
    REPORTS.with_borrow(|rows|assert!(rows.iter().any(|row|row=="retention-phase=0state")));
    REPORTS.with_borrow_mut(Vec::clear);
    assert_eq!(run(Mode::Delayed(251),250),Err("deadline".into()));
    REPORTS.with_borrow(|rows|assert!(rows.iter().any(|row|row=="retention-phase=2state")));
    REPORTS.with_borrow_mut(Vec::clear);
    assert_eq!(run(Mode::Success,250),Ok(()));
    REPORTS.with_borrow(|rows|assert!(rows.is_empty(),"success emitted failure diagnostics"));
}
