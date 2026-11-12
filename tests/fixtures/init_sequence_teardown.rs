use ::std::cell::RefCell;
use ::std::io::{ErrorKind, Read};

#[derive(Default)]
struct World {
    now: u64,
    events: Vec<String>,
    children: Vec<i32>,
    ready: Vec<i32>,
    stubborn: i32,
    immortal: bool,
    mount_stuck: bool,
    mounted: bool,
    output_stuck: bool,
    interrupted: bool,
}
thread_local! { static WORLD: RefCell<World> = RefCell::new(World::default()); }
macro_rules! print { ($($arg:tt)*) => { crate::event(format!($($arg)*)) }; }
fn event(text: String) { WORLD.with(|w| w.borrow_mut().events.push(text)); }
fn now_ms() -> u64 { WORLD.with(|w| w.borrow().now) }
mod std {
    pub mod process { pub fn id() -> u32 { 1 } }
    pub mod fs {
        pub fn read_to_string(path: &str) -> Result<String, ::std::io::Error> {
            crate::event(format!("read {path}"));
            Ok(match path {
                "/etc/shares" => "//site/share /shares/data smb credfile=/etc/fixture\n\
                    //site/other /shares/not-mounted smb credfile=/etc/fixture\n\
                    //site/local /local smb credfile=/etc/fixture\n".into(),
                "/proc/mounts" => crate::WORLD.with(|w| {
                    let mut mounts = "none / ext2 rw 0 0\nnone /local ext2 rw 0 0\n".to_string();
                    if w.borrow().mounted { mounts.push_str("//site/share /shares/data ufs rw 0 0\n"); }
                    mounts
                }),
                "/proc/pids" => crate::WORLD.with(|w| w.borrow().children.iter()
                    .map(|p| format!("{p}\n")).collect()),
                _ => panic!("unexpected read {path}"),
            })
        }
    }
}
mod libbreenix {
    #[derive(Debug, PartialEq, Eq)]
    pub enum Errno { ECHILD, EINTR, ESRCH }
    pub mod error {
        #[derive(Debug)]
        pub enum Error { Os(super::Errno) }
        impl ::std::fmt::Display for Error {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result { write!(f, "{self:?}") }
        }
    }
    pub mod time {
        pub struct Timespec;
        impl Timespec { pub fn as_nanos(&self) -> u128 { crate::now_ms() as u128 * 1_000_000 } }
        pub fn now_monotonic() -> Result<Timespec, super::error::Error> { Ok(Timespec) }
        pub fn sleep_ms(ms: u64) -> Result<(), super::error::Error> {
            crate::WORLD.with(|w| {
                let mut w = w.borrow_mut();
                w.now += ms;
                assert!(w.now <= 50_000, "teardown never returned");
            });
            Ok(())
        }
    }
    pub mod fs {
        pub fn umount2(path: &str, flags: u32) -> Result<(), super::error::Error> {
            assert_eq!(path, "/shares/data"); assert_eq!(flags, 0);
            crate::event(format!("umount {path}"));
            crate::WORLD.with(|w| { let mut w = w.borrow_mut(); if !w.mount_stuck { w.mounted = false; } });
            Ok(())
        }
    }
}
mod process {
    pub const WNOHANG: i32 = 1;
    pub struct Pid(i32);
    impl Pid { pub fn raw(&self) -> i32 { self.0 } }
    pub fn waitpid(pid: i32, status: &mut i32, flags: i32) -> Result<Pid, crate::libbreenix::error::Error> {
        assert_eq!((pid, flags), (-1, WNOHANG));
        crate::WORLD.with(|w| {
            let mut w = w.borrow_mut();
            if !w.interrupted {
                w.interrupted = true;
                return Err(crate::libbreenix::error::Error::Os(crate::libbreenix::Errno::EINTR));
            }
            if !w.ready.is_empty() {
                let pid = w.ready.remove(0);
                w.children.retain(|p| *p != pid);
                w.events.push(format!("reap {pid}")); *status = 0;
                Ok(Pid(pid))
            } else if w.children.is_empty() {
                w.events.push("ECHILD".into());
                Err(crate::libbreenix::error::Error::Os(crate::libbreenix::Errno::ECHILD))
            } else { Ok(Pid(0)) }
        })
    }
    pub fn wifsignaled(status: i32) -> bool { status & 127 != 0 }
    pub fn wtermsig(status: i32) -> i32 { status & 127 }
    pub fn wexitstatus(status: i32) -> i32 { status >> 8 }
}
mod signal {
    pub const SIGTERM: i32 = 15;
    pub const SIGKILL: i32 = 9;
    pub fn kill(pid: i32, sig: i32) -> Result<(), crate::libbreenix::error::Error> {
        crate::WORLD.with(|w| {
            let mut w = w.borrow_mut();
            let now = w.now;
            w.events.push(format!("signal {pid} {sig} at {now}"));
            if pid == -1 { assert!(!w.mounted, "sweep killed the mounted share's server"); }
            let targets: Vec<_> = w.children.iter().copied().filter(|p| pid == -1 || *p == pid).collect();
            if targets.is_empty() {
                return Err(crate::libbreenix::error::Error::Os(crate::libbreenix::Errno::ESRCH));
            }
            for target in targets {
                if target == w.stubborn && (sig != SIGKILL || w.immortal) { continue; }
                if !w.ready.contains(&target) { w.ready.push(target); }
            }
            Ok(())
        })
    }
}
mod smbfs {
    pub mod shares {
        pub struct Share { pub directory: String }
        pub struct ShareError { pub line: usize, pub message: &'static str }
        pub fn parse_shares(text: &str) -> (Vec<Share>, Vec<ShareError>) {
            (text.lines().map(|l| Share { directory: l.split_whitespace().nth(1).unwrap().into() }).collect(), vec![])
        }
    }
}
mod services {
    pub enum Need { Service(String), Entropy }
    pub struct AfterExit { pub restart: bool, pub crash_loop: bool, pub delay_ms: u64 }
    pub fn after_exit(_: (), _: u64, _: bool, _: u32) -> AfterExit {
        AfterExit { restart: true, crash_loop: false, delay_ms: 50 }
    }
}
use services::Need;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State { Waiting, Running, Stopping, Stopped, Backoff }
impl State {
    fn is_live(self) -> bool { matches!(self, Self::Running | Self::Stopping) }
}
enum AfterStop { Stay, Start, Remove }
struct Definition { name: String, needs: Vec<Need>, stop_timeout_ms: u64, restart: () }
struct Entry {
    def: Definition, state: State, pid: i32, after_stop: AfterStop,
    stop_deadline_ms: u64, killed: bool, last_exit: String, started_ms: u64,
    quick_failures: u32, crash_loop_reported: bool, failures: Failures,
    next_ms: u64, log: Option<Log>, outputs: Vec<Output>,
}
struct Failures;
impl Failures { fn clear(&mut self) {} }
impl Entry {
    fn set(&mut self, state: State, _: u64) { self.state = state; }
    fn answer(&mut self, _: bool, _: &str) {}
}
struct Notices;
impl Notices { fn say(&mut self, _: &str) {} }
struct Log(Vec<u8>);
impl Log {
    fn retry(&mut self) {}
    fn record(&mut self, text: &[u8]) { self.0.extend_from_slice(text); }
}
struct Lines(Vec<u8>);
impl Lines {
    fn finish(&mut self, _: &str, log: &mut Log) { log.record(&self.0); self.0.clear(); }
    fn feed_observed(&mut self, text: &[u8], _: &str, _: &mut Log, _: impl FnMut(&[u8])) { self.0.extend_from_slice(text); }
}
struct Reader;
impl Read for Reader {
    fn read(&mut self, _: &mut [u8]) -> ::std::io::Result<usize> {
        if WORLD.with(|w| w.borrow().output_stuck) { Err(ErrorKind::WouldBlock.into()) } else { Ok(0) }
    }
}
struct Output { reader: Reader, lines: Lines, stream: &'static str, ended: bool }
fn desktop_log_receipt(_: &[u8]) -> bool { false }
fn login_terminal(_: &Definition) -> Option<Option<&str>> { None }
const TICK_MS: u64 = 50;
struct Supervisor { entries: Vec<Entry>, changed: bool, notices: Notices }
impl Supervisor {
    fn write_state(&mut self) { event("publish".into()); }
    fn schedule(entry: &mut Entry, decision: services::AfterExit, _: u64, _: &mut Notices) {
        assert!(decision.restart); entry.state = State::Backoff;
    }
    // METHODS
}
// FUNCTIONS
fn supervisor() -> Supervisor {
    WORLD.with(|w| *w.borrow_mut() = World {
        children: vec![10, 20, 30, 99], stubborn: 20, mounted: true, ..World::default()
    });
    Supervisor {
        // Deliberately put blogd last and the leaf first: table reversal alone is wrong.
        entries: [("leaf", 30, vec![Need::Service("base".into())]),
                  ("base", 20, vec![Need::Entropy]), ("blogd", 10, vec![])].into_iter().map(|(name, pid, needs)| Entry {
            def: Definition { name: name.into(), needs, stop_timeout_ms: 150, restart: () },
            state: State::Running, pid, after_stop: AfterStop::Start, stop_deadline_ms: 0,
            killed: false, last_exit: String::new(), started_ms: 0, quick_failures: 0,
            crash_loop_reported: false, failures: Failures, next_ms: 0,
            log: Some(Log(vec![])), outputs: vec![Output {
                reader: Reader, lines: Lines(b"last partial line".to_vec()), stream: "stdout", ended: false,
            }],
        }).collect(), changed: false, notices: Notices,
    }
}
#[test]
fn dependency_stop_mount_readback_sweep_reap_and_log_drain() {
    let mut s = supervisor();
    assert_eq!(s.stop_for_sequence(), Ok(()));
    assert!(s.entries.iter().all(|e| e.state == State::Stopped && e.pid == 0 && e.outputs.is_empty()));
    assert!(s.entries.iter().all(|e| e.log.as_ref().unwrap().0 == b"last partial line"));
    WORLD.with(|w| {
        let w = w.borrow();
        let signals: Vec<_> = w.events.iter().filter(|e| e.starts_with("signal")).map(String::as_str).collect();
        assert_eq!(signals, ["signal 30 15 at 0", "signal 20 15 at 0", "signal 20 9 at 150",
                            "signal 10 15 at 200", "signal -1 15 at 200", "signal -1 9 at 2200"]);
        let at = |text: &str| w.events.iter().position(|e| e == text).unwrap();
        assert!(at("reap 10") < at("umount /shares/data"));
        let unmount = at("umount /shares/data");
        assert_eq!(w.events[unmount + 1], "read /proc/mounts");
        let receipt = at("[init] sequence shares readback pid=1 configured=3 mounted=0\n");
        assert!(unmount + 1 < receipt && receipt < at("signal -1 15 at 200"));
        assert!(at("reap 99") < w.events.iter().rposition(|e| e == "ECHILD").unwrap());
        assert!(w.children.is_empty());
    });
    // An in-flight remove/restart must become a final stop during handoff.
    let mut s = supervisor();
    s.entries[0].state = State::Stopping;
    s.entries[0].after_stop = AfterStop::Remove;
    s.entries[0].stop_deadline_ms = 150;
    WORLD.with(|w| w.borrow_mut().ready.push(30));
    assert_eq!(s.stop_for_sequence(), Ok(()));
    assert_eq!(s.entries.len(), 3);
    assert!(s.entries.iter().all(|e| e.state == State::Stopped));
}
#[test]
fn mount_left_after_successful_umount_is_an_error_before_sweep() {
    let mut s = supervisor();
    WORLD.with(|w| w.borrow_mut().mount_stuck = true);
    let error = s.stop_for_sequence().unwrap_err();
    assert!(error.contains("/shares/data"), "{error}");
    WORLD.with(|w| {
        let w = w.borrow();
        assert!(w.events.iter().any(|e| e == "[init] sequence shares readback pid=1 configured=3 mounted=1\n"));
        assert!(!w.events.iter().any(|e| e.starts_with("signal -1")));
    });
}
#[test]
fn service_that_outlives_sigkill_is_named_without_killing_share_server() {
    let mut s = supervisor();
    WORLD.with(|w| w.borrow_mut().immortal = true);
    let error = s.stop_for_sequence().unwrap_err();
    assert!(error.contains("base") && error.contains("20"), "{error}");
    WORLD.with(|w| assert!(!w.borrow().events.iter().any(|e| e.starts_with("signal -1"))));
}
#[test]
fn sweep_survivors_and_undrained_service_logs_are_named() {
    let mut s = supervisor();
    WORLD.with(|w| { let mut w = w.borrow_mut(); w.stubborn = 99; w.immortal = true; });
    let error = s.stop_for_sequence().unwrap_err();
    assert!(error.contains("99"), "{error}");
    let mut s = supervisor();
    WORLD.with(|w| w.borrow_mut().output_stuck = true);
    let error = s.stop_for_sequence().unwrap_err();
    assert!(error.contains("leaf") && error.contains("output"), "{error}");
}
