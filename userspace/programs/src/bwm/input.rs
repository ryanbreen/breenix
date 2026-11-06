//! Source swap from #2476 dfc2e318: /dev/input/events owns the compositor grab.
//! Content routing lives in pointer_route; chrome gestures remain in the caller.
use super::{input_backlog, input_keys, session, Input, Window};
use libbreenix::input::{
    self as core_input, axis, disposition as outcome, key, kind, reason, syn, Event,
};
use libbreenix::window::abi;
use std::collections::BTreeMap;

static READER: std::sync::OnceLock<core_input::Reader> = std::sync::OnceLock::new();
static SOURCE: std::sync::Mutex<Context> = std::sync::Mutex::new(Context {
    event: Event {
        serial: 0,
        time_ns: 0,
        device: 0,
        kind: 0,
        code: 0,
        value: 0,
        flags: 0,
    },
    derived: 0,
    accounted: false,
});
struct Context {
    event: Event,
    derived: u32,
    accounted: bool,
}
#[derive(Clone, Copy)]
struct Queued {
    event: abi::WindowEvent,
    source: u64,
    derived: u32,
}
static QUEUES: std::sync::Mutex<input_backlog::Backlog<Queued>> =
    std::sync::Mutex::new(input_backlog::Backlog::new());

fn dispose(serial: u64, derived: u32, window: u64, disposition: u32, reason: u32) {
    if serial == 0 {
        return;
    }
    let d = core_input::Disposition {
        serial,
        window,
        disposition,
        reason,
        derived,
        reserved: 0,
    };
    match READER.get().expect("input reader").dispose(&d) {
        Ok(()) => {}
        // Pressure recovery can terminate a source row before bwm drains it.
        Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EALREADY)) => {}
        Err(e) => eprintln!("[bwm] input disposition {}: {}", serial, e),
    }
}

pub(super) fn begin(event: Event) {
    *SOURCE.lock().unwrap() = Context {
        event,
        derived: 0,
        accounted: false,
    };
}
pub(super) fn reject(why: u32) {
    let mut context = SOURCE.lock().unwrap();
    dispose(context.event.serial, 0, 0, outcome::REJECTED, why);
    context.accounted = true;
}
pub(super) fn finish() {
    let mut context = SOURCE.lock().unwrap();
    if !context.accounted {
        dispose(
            context.event.serial,
            0,
            0,
            outcome::WM_HANDLED,
            reason::NONE,
        );
    }
    context.event = Event::default();
}
fn losses(queues: &mut input_backlog::Backlog<Queued>) {
    for (id, q, why) in queues.take_discarded() {
        let (id, why) = match why {
            input_backlog::DropReason::Overflow => (id, reason::OVERFLOW),
            input_backlog::DropReason::Gone => (0, reason::TARGET_GONE),
            input_backlog::DropReason::Failed => (id, reason::RING_FULL),
        };
        dispose(q.source, q.derived, id, outcome::LOST, why);
    }
}
pub(super) fn flush_input() {
    use input_backlog::Delivery;
    let mut queues = QUEUES.lock().unwrap();
    losses(&mut queues);
    queues.flush(
        |id, q| match session::send(id, q.event) {
            Ok(1) => {
                dispose(q.source, q.derived, id, outcome::DELIVERED, reason::NONE);
                Delivery::Accepted
            }
            Ok(_) | Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN)) => {
                Delivery::Full
            }
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ENOENT)) => Delivery::Gone,
            Err(e) => {
                eprintln!("[bwm] event delivery {}: {}", id, e);
                Delivery::Failed
            }
        },
        |lost| Queued {
            event: session::event(Input::Overflow { lost }),
            source: 0,
            derived: 0,
        },
    );
    losses(&mut queues);
}
pub(super) fn retain_windows(exists: impl FnMut(u64) -> bool) {
    let mut queues = QUEUES.lock().unwrap();
    queues.retain_windows(exists);
    losses(&mut queues);
}
fn enqueue(id: u64, q: Queued) {
    let mut queues = QUEUES.lock().unwrap();
    // Keep distinct source identities; coalescing policy belongs to 21.4.
    if queues.push(id, q, |_, _| false) == input_backlog::Admission::Disabled {
        if q.event.kind == abi::event_kind::CLOSE_REQUESTED {
            queues.request_close(id, q);
        } else {
            dispose(q.source, q.derived, id, outcome::LOST, reason::RING_FULL);
        }
    }
    drop(queues);
    flush_input();
}
pub(super) fn send_to(windows: &[Window], index: usize, input: Input) {
    let Some(w) = windows.get(index).filter(|w| w.window_id != 0) else {
        return;
    };
    send_id(w.window_id, input);
}
pub(super) fn send_id(id: u64, input: Input) {
    let mut event = session::event(input);
    let mut context = SOURCE.lock().unwrap();
    let source = context.event;
    let raw = !context.accounted
        && match input {
            Input::Move { .. } => {
                matches!(source.kind, kind::ABS | kind::REL) && source.code <= axis::Y
            }
            Input::Button { .. } => source.kind == kind::KEY && source.code >= key::BUTTON_LEFT,
            Input::Scroll { .. } => source.kind == kind::REL && source.code == axis::WHEEL,
            _ => false,
        };
    let derived = if raw || source.serial == 0 {
        0
    } else {
        context.derived += 1;
        context.derived
    };
    event.serial = if derived == 0 {
        source.serial
    } else {
        core_input::abi::derived_serial(source.serial, derived).expect("input derivation")
    };
    event.time_ns = source.time_ns;
    event.modifiers = source.flags;
    context.accounted |= raw;
    drop(context);
    enqueue(
        id,
        Queued {
            event,
            source: source.serial,
            derived,
        },
    );
}
pub(super) fn send_keys(windows: &[Window], stack: &super::Stack, source: Event, payload: &[u8]) {
    let Some(w) = stack.focus().and_then(|id| windows.iter().find(|w| w.window_id == id))
        .filter(|w| !stack.minimized(w.window_id)) else {
        reject(reason::NO_FOCUS);
        return;
    };
    let events = input_keys::events(source, payload);
    let mut context = SOURCE.lock().unwrap();
    context.accounted = !events.is_empty();
    context.derived = events.len().saturating_sub(1) as u32;
    drop(context);
    for event in events {
        let derived = core_input::abi::derived_source(event.serial).map_or(0, |(_, n)| n);
        enqueue(
            w.window_id,
            Queued {
                event,
                source: source.serial,
                derived,
            },
        );
    }
}

pub(super) struct Stream {
    capabilities: BTreeMap<u32, Vec<core_input::Capability>>,
    x: i32,
    y: i32,
    pub(super) modifiers: u8,
}
impl Stream {
    pub(super) fn new() -> Self {
        let reader = core_input::Reader::open(true).expect("open compositor input reader");
        reader.grab(&[]).expect("claim compositor input grab");
        assert!(READER.set(reader).is_ok(), "one input reader");
        Self {
            capabilities: BTreeMap::new(),
            x: 0,
            y: 0,
            modifiers: 0,
        }
    }
    pub(super) fn fd(&self) -> i32 {
        READER.get().unwrap().fd().raw() as i32
    }
    /// A zero-capacity capability query tests registration without copying entries.
    /// Preserve state on transient errors; only ENODEV marks the id as retired.
    pub(super) fn registered(&self, device: u32) -> bool {
        !matches!(READER.get().unwrap().capabilities(device, &mut []),
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ENODEV)))
    }
    pub(super) fn read(&self) -> Vec<(Event, Vec<u8>)> {
        let mut bytes = vec![0; core_input::abi::READER_EVENTS * (32 + 256)];
        match READER.get().unwrap().read(&mut bytes) {
            Ok(records) => records
                .map(|r| r.map(|r| (r.event, r.payload.to_vec())))
                .collect::<Result<Vec<_>, _>>()
                .expect("valid input batches"),
            Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::EAGAIN)) => Vec::new(),
            Err(e) => panic!("input stream: {}", e),
        }
    }
    fn absolute(&mut self, event: Event, extent: usize) -> Option<i32> {
        if !self.capabilities.contains_key(&event.device) {
            let mut caps = vec![core_input::Capability::default(); 512];
            let count = READER
                .get()
                .unwrap()
                .capabilities(event.device, &mut caps)
                .ok()?;
            if count > caps.len() {
                return None;
            }
            caps.truncate(count);
            caps.shrink_to_fit();
            // Device ids are not reused. Bound metadata retained from removed
            // virtual devices; a cache miss just queries the live descriptor.
            if self.capabilities.len() == 64 {
                self.capabilities.clear();
            }
            self.capabilities.insert(event.device, caps);
        }
        let cap = self.capabilities[&event.device]
            .iter()
            .find(|c| c.kind == kind::ABS && c.code == event.code)?;
        Some(input_keys::normalize(event.value, *cap, extent))
    }
    /// Normalize axes once, then route contiguous same-device axes at their final position.
    pub(super) fn positions(
        &mut self, records: &[(Event, Vec<u8>)], width: usize, height: usize,
    ) -> Vec<Option<(i32, i32)>> {
        use super::pointer_route::{batch_positions, Axis};
        let samples: Vec<_> = records.iter().map(|(event, _)| {
            let axis = match (event.kind, event.code) {
                (kind::ABS, axis::X) => self.absolute(*event, width).map(Axis::AbsoluteX),
                (kind::ABS, axis::Y) => self.absolute(*event, height).map(Axis::AbsoluteY),
                (kind::REL, axis::X) => Some(Axis::RelativeX(event.value)),
                (kind::REL, axis::Y) => Some(Axis::RelativeY(event.value)),
                _ => None,
            };
            (event.device, axis.unwrap_or(Axis::Boundary))
        }).collect();
        batch_positions(&samples, (self.x, self.y), (width, height))
    }
    /// Feed the normalized snapshot to content routing and the existing chrome policy.
    pub(super) fn pointer(
        &mut self, event: Event, position: Option<(i32, i32)>,
    ) -> Option<(i32, i32)> {
        match (event.kind, event.code) {
            (kind::ABS | kind::REL, axis::X | axis::Y) => {
                (self.x, self.y) = position?;
            }
            (kind::REL, axis::WHEEL)
            | (kind::KEY, key::BUTTON_LEFT | key::BUTTON_RIGHT | key::BUTTON_MIDDLE)
            | (kind::SYN, syn::DROPPED) => {}
            _ => return None,
        }
        Some((self.x, self.y))
    }
}

pub(super) fn send_repeat(repeat: super::timing::Repeat) {
    let mut event = input_keys::events(repeat.source, &[]).remove(0);
    event.flags |= abi::EVENT_REPEAT;
    event.serial = core_input::abi::derived_serial(repeat.source.serial, repeat.derived).expect("input derivation");
    event.time_ns = repeat.time;
    enqueue(repeat.target, Queued { event, source: repeat.source.serial, derived: repeat.derived });
}
