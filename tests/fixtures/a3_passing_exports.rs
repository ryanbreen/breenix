use std::cell::RefCell;
type CaseResult = Result<(), String>;
const ANIMATE: u32 = 1;
const TRACE: &str = "account-input frozen=1 begin_ns=100 end_ns=80000100\n";

#[derive(Default)]
struct Fixture {
    clock: u64, latency: u64, begin: u64, serial: u64,
    read_error: bool, write_error: bool, observed: bool,
    output: Vec<u8>, values: Vec<(String, i64, String, Option<(i64, i64)>)>,
}
thread_local! { static FIXTURE: RefCell<Fixture> = RefCell::new(Fixture::default()); }
fn now() -> u64 { FIXTURE.with(|f| f.borrow().clock) }
fn check(ok: bool, error: &str) -> CaseResult { if ok { Ok(()) } else { Err(error.into()) } }
fn value(name: &str, n: i64, unit: &str, bound: Option<(i64, i64)>) {
    FIXTURE.with(|f| f.borrow_mut().values.push((name.into(), n, unit.into(), bound)));
}
fn failure_pipeline() {}
mod time {
    pub fn sleep_ms(ms: u64) -> Result<(), String> {
        super::FIXTURE.with(|f| f.borrow_mut().clock += ms * 1_000_000); Ok(())
    }
}
mod kind { pub const POINTER_MOVE: u32 = 1; pub const KEY_PRESS: u32 = 2; }
struct Event { kind: u32 }
struct Geometry { x: i32, y: i32 }
struct Client { id: u64, handling_commit: u64 }
impl Client {
    fn start(_: u64) -> Result<Self, String> { Ok(Self { id: 42, handling_commit: 1 }) }
    fn command(&mut self, _: u32, _: u64, _: u64) -> CaseResult { Ok(()) }
    fn clear(&mut self) -> CaseResult { Ok(()) }
    fn info(&self) -> Result<Geometry, String> { Ok(Geometry { x: 10, y: 10 }) }
    fn await_event(&mut self, _: &str, accept: impl Fn(&Event) -> bool) -> CaseResult {
        assert!(accept(&Event { kind: kind::KEY_PRESS }) || accept(&Event { kind: kind::POINTER_MOVE }));
        Ok(())
    }
}
struct Desktop { width: u32, height: u32 }
impl Desktop {
    fn start() -> Result<Self, String> { Ok(Self { width: 1024, height: 768 }) }
    fn shown(&mut self, _: &Client) -> CaseResult {
        let mut observation = WaitObservation::default();
        observation.read()?;
        let _ = observation.failure();
        Ok(())
    }

}
struct Virtual;
impl Virtual {
    fn create() -> Result<Self, String> { Ok(Self) }
    fn batch(&mut self, _: &[(u16, u16, i32)], begin: u64) -> CaseResult {
        FIXTURE.with(|f| f.borrow_mut().begin = begin); Ok(())
    }
}
mod measure {
    use super::*;
    pub struct Receipt { pub ns: u64, pub serial: u64 }
    pub fn receipts() -> CaseResult { Ok(()) }
    pub fn completed_commit(_: &Client, _: u64) -> Result<Receipt, String> {
        FIXTURE.with(|f| {
            let mut f = f.borrow_mut(); f.clock = f.begin + f.latency; f.serial += 1;
            Ok(Receipt { ns: f.clock, serial: f.serial })
        })
    }
    pub fn observed(_: &mut Desktop, _: &[&Client], serial: u64) -> CaseResult {
        assert_eq!(serial, 110); FIXTURE.with(|f| f.borrow_mut().observed = true); Ok(())
    }
}
mod guest_fs {
    use std::io::{self, Write};
    pub struct File;
    impl File {
        pub fn open(path: &str) -> io::Result<io::Cursor<Vec<u8>>> {
            assert_eq!(path, "/etc/breenix/evidence-route");
            Err(io::Error::from(io::ErrorKind::NotFound))
        }
    }
    pub fn read_to_string(path: &str) -> io::Result<String> {
        if path == "/proc/breenix/window-readback" {
            return Ok("begin_ns=1 end_ns=1 branch=busy observer=1 tid=1 claimant=1 epoch=1 first=1 serial=1 busy=0 completed=1 flags=0 ack=1\n".repeat(2048));
        }
        if path == "/proc/breenix/window-pipeline-full" {
            return Ok("10 1 1 1 1\n".repeat(2048));
        }
        assert_eq!(path, "/proc/breenix/window-stall");
        if super::FIXTURE.with(|f| f.borrow().read_error) {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        Ok(super::TRACE.into())
    }
    pub struct OpenOptions;
    impl OpenOptions {
        pub fn new() -> Self { Self }
        pub fn write(&mut self, write: bool) -> &mut Self { assert!(write); self }
        pub fn open(&self, path: &str) -> io::Result<Sink> {
            assert_eq!(path, "/dev/screen-evidence"); Ok(Sink)
        }
    }
    pub struct Sink;
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            super::FIXTURE.with(|f| {
                let mut f = f.borrow_mut();
                if f.write_error && bytes.starts_with(b"WM_STALL ") {
                    return Err(io::Error::from(io::ErrorKind::PermissionDenied));
                }
                f.output.extend_from_slice(bytes); Ok(bytes.len())
            })
        }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }
}
mod libbreenix { pub mod crypto { pub mod sha256 { pub fn sha256(_: &[u8]) -> [u8;32] { panic!("no route") } } } }
mod breenix_suite { pub mod suite {
    use crate::libbreenix;
    mod evidence {
        pub const MAX_ROUTE: usize = 4096;
        pub const MAX_UPLOAD: usize = 1 << 24;
        pub struct Route { pub path: String }
        impl Route {
            pub fn parse(_: &str, _: &str, _: bool) -> Result<Self, String> { panic!("no route") }
        }
    }
    mod sequence { pub fn read(_: &str) -> Option<(Vec<String>, ())> { None } }
    fn evidence_transfer(_: &evidence::Route, _: &str, _: &[u8], _: [u8;32]) -> Result<(), String> { panic!("no route") }
    pub fn running_case() -> Option<&'static str> { Some("performance/latency") }
    pub fn case_budget(_: &str, _: &str) {}
    ROUTE_BODY
    UPLOAD_BODY
} }
fn word(_: &[u8], _: usize) -> u32 { 0 }
fn quad(_: &[u8], _: usize) -> u64 { 0 }
struct Screen { record: [u8; 1024], width: u32 }
impl Screen {
    fn read() -> Result<Self, String> { Ok(Self { record: [0; 1024], width: 1024 }) }
    fn pixel(&self, _: i32, _: i32) -> Result<u32, String> { Ok(0) }
}
#[derive(Default)]
WAIT_STRUCT
WAIT_IMPL
WAIT_DROP
DIAGNOSTIC_BODY
OBSERVATION_BODY
LATENCY_BODY


#[test]
fn passing_cases_keep_serial_exports_below_two_kibibytes() {
    for (pointer, animation) in [(false, false), (true, false), (false, true)] {
        FIXTURE.with(|f| *f.borrow_mut() = Fixture { clock: 1, latency: 12_000_000, ..Fixture::default() });
        assert_eq!(input_latency(pointer, animation), Ok(()));
        FIXTURE.with(|f| {
            let f = f.borrow(); assert!(f.observed);
            let output = String::from_utf8(f.output.clone()).unwrap();
            let bytes: usize = output.lines().filter(|l| l.starts_with("VALUE wm-")).map(|l| l.len() + 1).sum();
            assert!(bytes <= 2048, "passing pointer={pointer} animation={animation} exported {bytes} VALUE wm- bytes (limit 2048)");
        });
    }
}
