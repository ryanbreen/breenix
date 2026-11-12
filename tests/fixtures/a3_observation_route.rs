use std::cell::RefCell;
type CaseResult = Result<(), String>;
const ANIMATE: u32 = 1;
const TRACE: &str = "account-input frozen=1 begin_ns=100 end_ns=80000100\n";
const LATENCY_ERROR: &str = "input-to-composited-ack latency exceeds the GUI plan bound";
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
    fn shown(&mut self, _: &Client) -> CaseResult { Ok(()) }
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
DIAGNOSTIC_BODY
OBSERVATION_BODY
LATENCY_BODY

#[test]
fn no_route_preserves_latency_verdict_and_reports_observation_disposition() {
    for (pointer, animation) in [(false, false), (true, false), (false, true)] {
        for latency in [12_000_000, 110_500_000] {
            FIXTURE.with(|f| *f.borrow_mut() = Fixture { clock: 1, latency, ..Fixture::default() });
            let verdict = input_latency(pointer, animation);
            assert_eq!(verdict, if latency == 12_000_000 { Ok(()) } else { Err(LATENCY_ERROR.into()) });
            FIXTURE.with(|f| {
                let f = f.borrow(); assert!(f.observed);
                let bounds = if animation { (33000, 100000) } else { (20000, 50000) };
                for (name, bound) in [("latency-median", bounds.0), ("latency-max", bounds.1)] {
                    assert!(f.values.contains(&(name.into(), (latency / 1000) as i64, "us".into(), Some((0, bound)))));
                }
                let output = String::from_utf8(f.output.clone()).unwrap();
                assert!(output.contains("skipped=diagnostic-route-absent"), "{output}");
                let label = if animation { "input-animation-observation" } else if pointer { "input-pointer-observation" } else { "input-typing-observation" };
                assert!(output.contains(&format!("client_window=42 label={label}")), "{output}");
            });
        }
    }
    // Failed observation commands or reads must not become latency failures either.
    for (read_error, write_error) in [(true, false), (false, true)] {
        FIXTURE.with(|f| *f.borrow_mut() = Fixture { clock: 1, latency: 12_000_000, read_error, write_error, ..Fixture::default() });
        assert_eq!(input_latency(false, true), Ok(()));
        FIXTURE.with(|f| {
            let f = f.borrow(); assert!(f.observed);
            assert!(String::from_utf8_lossy(&f.output).contains("skipped="));
        });
    }
}
