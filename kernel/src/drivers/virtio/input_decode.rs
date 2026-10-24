//! Clean-room normalization of the virtio-input 1.2 eight-byte event format.
//! These are wire numbers, not code imported from another implementation.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Event {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

pub fn decode(bytes: [u8; 8]) -> Option<Event> {
    let event = Event {
        kind: u16::from_le_bytes([bytes[0], bytes[1]]),
        code: u16::from_le_bytes([bytes[2], bytes[3]]),
        value: i32::from_le_bytes(bytes[4..8].try_into().unwrap()),
    };
    let valid = match (event.kind, event.code) {
        (0, 0) => event.value == 0,
        (1, 1..=255 | 0x110..=0x112) => (0..=2).contains(&event.value),
        (2, 0 | 1 | 6 | 8) => true,
        (3, 0 | 1) => true,
        _ => false,
    };
    valid.then_some(event)
}

pub const FRAME_EVENTS: usize = 64;
/// One device's partial frame survives completion/IRQ boundaries. An overlong
/// frame is replaced by SYN_DROPPED at its boundary rather than a partial batch.
pub struct Frame {
    events: [Event; FRAME_EVENTS],
    len: usize,
    overflow: bool,
}
impl Frame {
    pub const fn new() -> Self {
        Self {
            events: [Event {
                kind: 0,
                code: 0,
                value: 0,
            }; FRAME_EVENTS],
            len: 0,
            overflow: false,
        }
    }
    pub fn push(&mut self, event: Event) -> Option<(&[Event], bool)> {
        if self.len < FRAME_EVENTS {
            self.events[self.len] = event;
            self.len += 1;
        } else {
            self.overflow = true;
        }
        if event.kind != 0 {
            return None;
        }
        let len = self.len;
        self.len = 0;
        let overflow = core::mem::replace(&mut self.overflow, false);
        Some((&self.events[..len], overflow))
    }
}
