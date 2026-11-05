//! Execute the production task/IRQ boundary with only the bus and sink replaced.
extern crate alloc;
extern crate self as input_abi;
pub use libbreenix::input::abi::{axis, key, kind, syn, Capability};
use std::sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Mutex};
#[path = "../kernel/src/input/host.rs"]
pub mod host;
pub use host::{Ack, Channel, Packet, BACKLOG, QUEUED};
#[path = "../kernel/src/input/host/runtime.rs"]
pub mod runtime;
#[path = "../kernel/src/drivers/hid_boot/report.rs"]
pub mod report;
pub mod drivers { pub mod hid_boot { pub use crate::report; } }
pub mod window { pub mod events { pub fn now_ns() -> u64 { crate::NOW.load(crate::Ordering::Relaxed) } } }
static NOW: AtomicU64 = AtomicU64::new(123);
static ACKS: Mutex<Vec<String>> = Mutex::new(Vec::new());
#[macro_export]
macro_rules! serial_println { ($($arg:tt)*) => { $crate::ACKS.lock().unwrap().push(format!($($arg)*)) }; }
pub mod input {
    #[derive(Clone, Copy, Debug)] pub enum Device { HostKeyboard }
    pub mod pointer { pub fn screen() -> Option<(u32,u32)> { Some((2560,1600)) } }
    pub mod hardware {
        #[derive(Clone, Copy, Debug)] pub struct Device { pub id: u32, pub keyboard: Option<super::Device> }
        pub struct Description { pub capabilities: Vec<crate::Capability>, pub keyboard: Option<super::Device> }
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub struct Event { pub kind: u16, pub code: u16, pub value: i32 }
    }
    pub mod stream {
        use super::hardware::{Description, Device, Event};
        use crate::{AtomicBool, Mutex, Ordering};
        pub static DEVICES: Mutex<Vec<Description>> = Mutex::new(Vec::new());
        pub static EVENTS: Mutex<Vec<(u32, Vec<Event>)>> = Mutex::new(Vec::new());
        pub static BLOCKED: AtomicBool = AtomicBool::new(false);
        pub fn register_hardware(d: Description) -> Result<Device, &'static str> {
            let mut devices = DEVICES.lock().unwrap();
            let device = Device { id: devices.len() as u32 + 1, keyboard: d.keyboard };
            devices.push(d); Ok(device)
        }
        pub fn wake_service() {}
        pub fn notify_host() {}
        pub fn drain_hardware(d: Device, events: &[Event], _: u64, lost: bool) -> bool {
            assert!(!lost);
            if BLOCKED.load(Ordering::Relaxed) { return false; }
            EVENTS.lock().unwrap().push((d.id, events.to_vec())); true
        }
    }
}

#[test]
fn runtime_publication_preserves_edges_under_backpressure_and_duplicate_acks() {
    use input::stream::{BLOCKED, DEVICES, EVENTS};
    let wire = b"BXKEY @0000000000000001:8cdc16e0 0000040000000000,0000000000000000";
    runtime::serial_line(wire); // undeclared: nothing accepted
    assert!(!runtime::drain_one());
    runtime::init().unwrap();
    runtime::init().unwrap();
    let devices = DEVICES.lock().unwrap();
    assert_eq!(devices.len(), 2);
    assert!(devices[0].keyboard.is_some());
    assert!(devices[1].keyboard.is_none());
    for (code, minimum, maximum) in [(axis::X,0,32767), (axis::Y,0,32767)] {
        assert!(devices[1].capabilities.iter().any(|c| c.kind == kind::ABS && c.code == code && c.minimum == minimum && c.maximum == maximum));
    }
    for code in [key::BUTTON_LEFT, key::BUTTON_RIGHT, key::BUTTON_MIDDLE] {
        assert!(devices[1].capabilities.iter().any(|c| c.kind == kind::KEY && c.code == code));
    }
    assert!(devices[1].capabilities.iter().any(|c| c.kind == kind::REL && c.code == axis::WHEEL));
    drop(devices);
    BLOCKED.store(true, Ordering::Relaxed);
    runtime::serial_line(wire);
    assert!(!runtime::drain_one());
    assert_eq!(*ACKS.lock().unwrap(), ["\nBXKEY_ACK 0000000000000001"]);
    assert!(EVENTS.lock().unwrap().is_empty());
    // ACK follows queue admission; pressure must not lose the decoded frame.
    runtime::serial_line(wire);
    BLOCKED.store(false, Ordering::Relaxed);
    assert!(runtime::drain_one());
    assert_eq!(ACKS.lock().unwrap().len(), 2);
    assert!(!runtime::drain_one()); // the old 10 ms report spacing remains
    NOW.fetch_add(10_000_000, Ordering::Relaxed);
    assert!(runtime::drain_one());
    assert!(!runtime::drain_one());
    let log = EVENTS.lock().unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!((log[0].0, log[0].1[0].kind, log[0].1[0].code, log[0].1[0].value), (1, kind::KEY, key::A, 1));
    assert_eq!(log[1].1[0].value, 0);
    assert!(log.iter().all(|(_, events)| events.last().unwrap().kind == kind::SYN && events.last().unwrap().code == syn::REPORT));
    drop(log);
    NOW.fetch_add(10_000_000, Ordering::Relaxed);
    let vector = include_str!("fixtures/bxptr-vectors.txt").lines().find(|l| l.contains("accept | BXPTR @0000000000000001")).unwrap().split_once(" | ").unwrap().1;
    runtime::serial_line(vector.as_bytes());
    assert!(runtime::drain_one());
    let log = EVENTS.lock().unwrap();
    let (device, e) = &log[2];
    assert_eq!(*device, 2);
    assert_eq!((e[0].kind, e[0].code, e[0].value), (kind::ABS, axis::X, 0));
    assert_eq!((e[1].kind, e[1].code, e[1].value), (kind::ABS, axis::Y, 0));
    assert!(e.iter().any(|e| (e.kind,e.code,e.value) == (kind::KEY,key::BUTTON_LEFT,1)));
    assert!(e.iter().any(|e| (e.kind,e.code,e.value) == (kind::KEY,key::BUTTON_MIDDLE,1)));
    assert!(e.iter().any(|e| (e.kind,e.code,e.value) == (kind::REL,axis::WHEEL,-1)));
    drop(log);
    runtime::serial_line(vector.as_bytes());
    assert!(runtime::drain_one()); // duplicate ACK, no duplicate input
    assert_eq!(EVENTS.lock().unwrap().len(), 3);
    runtime::serial_line(b"BXKEY @0000000000000002:602d0d99 0000000000000000");
    runtime::serial_line(b"BXKEY @0000000000000003:8f7fbb78 0000000000000000");
    while runtime::drain_one() {}
    let acks = ACKS.lock().unwrap();
    assert!(acks.iter().any(|a| a == "\nBXKEY_ACK 0000000000000002"));
    assert!(acks.iter().any(|a| a == "\nBXKEY_ACK 0000000000000003"));

}
