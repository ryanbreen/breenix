//! Fixed-storage driver ingress, harvested from e753f9a2 and shared by USB HID
//! (both architectures) and ARM virtio-input. Only the task service allocates.
use super::stream;
use alloc::vec::Vec;
use input_abi::{kind, syn, Capability};

pub const FRAME_EVENTS: usize = 64;
const FRAMES: usize = 64;
pub(super) const DEVICE_LIMIT: usize = 1024;

/// Decoded driver event, independent of any bus or wire representation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Event {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

/// The driver supplies capabilities only after binding the physical instance.
pub struct Description {
    pub capabilities: Vec<Capability>,
    pub keyboard: Option<super::Device>,
}

/// Non-reused hardware identity and its console source, retained by the pipe.
#[derive(Clone, Copy)]
pub struct Device {
    pub id: u32,
    pub keyboard: Option<super::Device>,
}

#[derive(Clone, Copy)]
struct Batch {
    device: Device,
    len: usize,
    events: [Event; FRAME_EVENTS],
    time_ns: u64,
    dropped: bool,
    sequence: u64,
}
const EMPTY: Batch = Batch {
    device: Device {
        id: 0,
        keyboard: None,
    },
    len: 0,
    events: [Event {
        kind: 0,
        code: 0,
        value: 0,
    }; FRAME_EVENTS],
    time_ns: 0,
    dropped: false,
    sequence: 0,
};
struct Ingress {
    batches: [Batch; FRAMES],
    head: usize,
    count: usize,
    dropped: [Option<Device>; DEVICE_LIMIT],
    removed: [Option<(Device, u64)>; DEVICE_LIMIT],
    accepted: u64,
    completed: u64,
    flight: Option<Batch>,
}
static INGRESS: spin::Mutex<Ingress> = spin::Mutex::new(Ingress {
    batches: [EMPTY; FRAMES],
    head: 0,
    count: 0,
    dropped: [None; DEVICE_LIMIT],
    removed: [None; DEVICE_LIMIT],
    accepted: 0,
    completed: 0,
    flight: None,
});

/// No allocation, logging, scheduler lock or task-context input lock in IRQs.
/// Drivers submit complete SYN_REPORT batches; an overlong frame becomes loss.
pub fn report(device: Device, events: &[Event], overflow: bool) {
    if !crate::test_control::input_report_allowed(device.id) { return; }
    crate::arch_without_interrupts(|| {
        let mut queue = INGRESS.lock();
        let id = device.id as usize;
        if queue.count == FRAMES {
            queue.dropped[id] = Some(device);
        } else {
            let at = (queue.head + queue.count) % FRAMES;
            let dropped = queue.dropped[id].take().is_some();
            queue.accepted += 1;
            let sequence = queue.accepted;
            let batch = &mut queue.batches[at];
            *batch = EMPTY;
            batch.device = device;
            batch.sequence = sequence;
            batch.dropped = dropped;
            batch.time_ns = crate::window::events::now_ns();
            if overflow || events.len() > FRAME_EVENTS {
                batch.len = 2;
                batch.events[0] = Event {
                    kind: kind::SYN,
                    code: syn::DROPPED,
                    value: 0,
                };
            } else {
                batch.len = events.len();
                batch.events[..events.len()].copy_from_slice(events);
            }
            queue.count += 1;
        }
    });
    stream::wake_service();
}

/// Stop reporting first. Drain accepted reports before publishing removal releases.
pub fn unregister(device: Device) {
    crate::arch_without_interrupts(|| {
        let mut queue = INGRESS.lock();
        let after = queue.accepted;
        queue.removed[device.id as usize] = Some((device, after));
    });
    stream::wake_service();
}

pub(super) fn drain_one() -> bool {
    let mut removal = None;
    let next = crate::arch_without_interrupts(|| {
        let mut queue = INGRESS.lock();
        if let Some(batch) = queue.flight {
            return Some(batch);
        }
        // A detach is a barrier at its accepted-report sequence. In particular,
        // an old port's releases precede a replacement keyboard's first press.
        let due = queue
            .removed
            .iter()
            .flatten()
            .filter(|(_, after)| *after <= queue.completed)
            .min_by_key(|(_, after)| *after)
            .map(|(device, _)| *device);
        let batch = if let Some(device) = due {
            if queue.dropped[device.id as usize].take().is_none() {
                removal = Some(device);
                return None;
            }
            loss(device, queue.completed)
        } else if queue.count != 0 {
            let batch = queue.batches[queue.head];
            queue.head = (queue.head + 1) % FRAMES;
            queue.count -= 1;
            batch
        } else if let Some(device) = queue.dropped.iter_mut().find_map(Option::take) {
            loss(device, queue.completed)
        } else {
            return None;
        };
        queue.flight = Some(batch);
        Some(batch)
    });
    if let Some(batch) = next {
        if !stream::drain_hardware(
            batch.device,
            &batch.events[..batch.len],
            batch.time_ns,
            batch.dropped,
        ) {
            return false;
        }
        crate::arch_without_interrupts(|| {
            let mut queue = INGRESS.lock();
            queue.completed = batch.sequence;
            queue.flight = None;
        });
        true
    } else if let Some(device) = removal {
        if !stream::remove_hardware(device) {
            return false;
        }
        crate::arch_without_interrupts(|| INGRESS.lock().removed[device.id as usize] = None);
        true
    } else {
        false
    }
}

fn loss(device: Device, sequence: u64) -> Batch {
    let mut batch = EMPTY;
    batch.device = device;
    batch.sequence = sequence;
    batch.len = 2;
    batch.events[0] = Event {
        kind: kind::SYN,
        code: syn::DROPPED,
        value: 0,
    };
    batch.time_ns = crate::window::events::now_ns();
    batch
}
