//! Fixed-storage handoff from the MMIO IRQ to the task-context input core.
use super::stream;
use crate::drivers::virtio::input_decode::{Event, FRAME_EVENTS};
use input_abi::{kind, syn};

pub const KEYBOARD: u32 = 2;
pub const TABLET: u32 = 3;
const FRAMES: usize = 64;
#[derive(Clone, Copy)]
struct Batch {
    device: u32,
    len: usize,
    events: [Event; FRAME_EVENTS],
    wake: bool,
    time_ns: u64,
    dropped: bool,
}
const EMPTY: Batch = Batch {
    device: 0,
    len: 0,
    events: [Event {
        kind: 0,
        code: 0,
        value: 0,
    }; FRAME_EVENTS],
    wake: true,
    time_ns: 0,
    dropped: false,
};
struct Ingress {
    batches: [Batch; FRAMES],
    head: usize,
    count: usize,
    dropped: u8,
    flight: Option<Batch>,
}
static INGRESS: spin::Mutex<Ingress> = spin::Mutex::new(Ingress {
    batches: [EMPTY; FRAMES],
    head: 0,
    count: 0,
    dropped: 0,
    flight: None,
});

/// No allocation, logging, scheduler lock or task-context input lock here.
pub fn report(device: u32, events: &[Event], overflow: bool, wake: bool) {
    crate::arch_without_interrupts(|| {
        let mut queue = INGRESS.lock();
        if queue.count == FRAMES {
            queue.dropped |= 1 << device;
        } else {
            let at = (queue.head + queue.count) % FRAMES;
            let dropped = queue.dropped & (1 << device) != 0;
            queue.dropped &= !(1 << device);
            let batch = &mut queue.batches[at];
            batch.dropped = dropped;
            batch.device = device;
            batch.wake = wake;
            batch.time_ns = crate::window::events::now_ns();
            if overflow {
                batch.len = 2;
                batch.events[0] = Event {
                    kind: kind::SYN,
                    code: syn::DROPPED,
                    value: 0,
                };
                batch.events[1] = Event::default();
            } else {
                batch.len = events.len();
                batch.events[..events.len()].copy_from_slice(events);
            }
            queue.count += 1;
        }
    });
    stream::wake_service();
}
fn peek(queue: &Ingress) -> Option<Batch> {
    if let Some(b) = queue.flight {
        Some(b)
    } else if queue.count != 0 {
        Some(queue.batches[queue.head])
    } else if queue.dropped != 0 {
        let mut b = EMPTY;
        b.device = queue.dropped.trailing_zeros();
        b.len = 2;
        b.events[0] = Event {
            kind: kind::SYN,
            code: syn::DROPPED,
            value: 0,
        };
        b.time_ns = crate::window::events::now_ns();
        Some(b)
    } else {
        None
    }
}
pub fn ready() -> bool {
    let next = crate::arch_without_interrupts(|| peek(&INGRESS.lock()));
    next.is_some_and(|b| stream::hardware_capacity(b.len + usize::from(b.dropped)))
}
pub fn drain_one() -> bool {
    let next = crate::arch_without_interrupts(|| {
        let mut queue = INGRESS.lock();
        let batch = peek(&queue)?;
        if queue.flight.is_none() {
            if queue.count != 0 {
                queue.head = (queue.head + 1) % FRAMES;
                queue.count -= 1;
            } else {
                queue.dropped &= !(1 << batch.device);
            }
            queue.flight = Some(batch);
        }
        Some(batch)
    });
    let Some(batch) = next else {
        return false;
    };
    if !stream::publish_hardware(
        batch.device,
        &batch.events[..batch.len],
        batch.time_ns,
        batch.wake,
        batch.dropped,
    ) {
        return false;
    }
    crate::arch_without_interrupts(|| INGRESS.lock().flight = None);
    true
}
