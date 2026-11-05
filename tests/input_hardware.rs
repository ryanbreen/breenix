//! Exercise the production handoff under pressure without a guest or a bus.
extern crate alloc;
extern crate self as input_abi;
pub use libbreenix::input::abi::{kind, syn, Capability};
use std::sync::Mutex;

#[derive(Clone, Copy)]
pub enum Device {
    Keyboard,
}
#[path = "../kernel/src/input/hardware.rs"]
pub mod hardware;
pub mod input {
    pub use crate::hardware;
}
#[path = "../kernel/src/drivers/virtio/input_decode.rs"]
pub mod input_decode;
fn arch_without_interrupts<T>(f: impl FnOnce() -> T) -> T {
    f()
}
mod test_control { pub fn input_report_allowed(_:u32) -> bool { true } }
mod window {
    pub mod events {
        pub fn now_ns() -> u64 {
            123
        }
    }
}
mod stream {
    use super::{
        hardware::{Device, Event},
        Mutex,
    };
    pub static LOG: Mutex<Vec<(u32, Vec<Event>, bool)>> = Mutex::new(Vec::new());
    pub static REMOVED: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    pub static BLOCKED: Mutex<bool> = Mutex::new(false);
    pub fn wake_service() {}
    pub fn drain_hardware(d: Device, events: &[Event], stamp: u64, lost: bool) -> bool {
        assert_eq!(stamp, 123);
        if *BLOCKED.lock().unwrap() {
            return false;
        }
        LOG.lock().unwrap().push((d.id, events.to_vec(), lost));
        true
    }
    pub fn remove_hardware(d: Device) -> bool {
        REMOVED.lock().unwrap().push(d.id);
        LOG.lock().unwrap().push((d.id, Vec::new(), false));
        true
    }
}
#[test]
fn handoff_retries_whole_frames_and_orders_loss_before_removal() {
    use hardware::{Device, Event};
    let device = Device {
        id: 300,
        keyboard: None,
    };
    let frame = [
        Event {
            kind: 1,
            code: 30,
            value: 1,
        },
        Event::default(),
    ];
    for _ in 0..64 {
        hardware::report(device, &frame, false);
    }
    hardware::report(device, &frame, false); // full: retain a loss marker
    hardware::unregister(device);
    *stream::BLOCKED.lock().unwrap() = true;
    assert!(!hardware::drain_one());
    assert!(stream::LOG.lock().unwrap().is_empty());
    assert!(stream::REMOVED.lock().unwrap().is_empty());
    *stream::BLOCKED.lock().unwrap() = false;
    while hardware::drain_one() {}
    let log = stream::LOG.lock().unwrap();
    assert_eq!(log.len(), 66);
    assert!(log[..64]
        .iter()
        .all(|(id, events, lost)| *id == 300 && *events == frame && !lost));
    assert_eq!(
        log[64].1,
        [
            Event {
                kind: 0,
                code: 3,
                value: 0
            },
            Event::default()
        ]
    );
    drop(log);
    assert_eq!(*stream::REMOVED.lock().unwrap(), [300]);

    hardware::report(
        Device {
            id: 301,
            keyboard: None,
        },
        &[Event::default(); hardware::FRAME_EVENTS + 1],
        false,
    );
    assert!(hardware::drain_one());
    assert_eq!(stream::LOG.lock().unwrap().last().unwrap().1[0].code, 3);
    assert!(!hardware::drain_one());

    let replacement = Device {
        id: 302,
        keyboard: None,
    };
    hardware::report(replacement, &frame, false);
    hardware::unregister(replacement);
    hardware::report(
        Device {
            id: 303,
            keyboard: None,
        },
        &frame,
        false,
    );
    while hardware::drain_one() {}
    let log = stream::LOG.lock().unwrap();
    let tail = &log[log.len() - 3..];
    assert_eq!(
        tail.iter()
            .map(|(id, events, _)| (*id, events.is_empty()))
            .collect::<Vec<_>>(),
        [(302, false), (302, true), (303, false)]
    );
}

#[test]
fn virtio_partial_frames_survive_completion_boundaries() {
    let mut frame = input_decode::Frame::new();
    let key = input_decode::decode([1, 0, 30, 0, 1, 0, 0, 0]).unwrap();
    assert!(frame.push(key).is_none());
    let (events, overflow) = frame.push(hardware::Event::default()).unwrap();
    assert!(!overflow);
    assert_eq!(events, [key, hardware::Event::default()]);
    for _ in 0..=hardware::FRAME_EVENTS {
        assert!(frame.push(key).is_none());
    }
    assert!(frame.push(hardware::Event::default()).unwrap().1);
    assert!(!frame.push(hardware::Event::default()).unwrap().1);
}
