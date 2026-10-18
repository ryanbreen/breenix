//! Fixed storage for keyboard bytes awaiting the terminal's line discipline.
//! A busy discipline keeps the oldest byte queued; no byte reaches a reader raw.
//! Each byte keeps its source, which the console terminal records on admission.
use super::InputSource;

pub(super) struct PendingInput {
    bytes: [(u8, InputSource); 256],
    head: usize,
    len: usize,
}

impl PendingInput {
    pub(super) const fn new() -> Self {
        Self {
            bytes: [(0, InputSource::Other); 256],
            head: 0,
            len: 0,
        }
    }

    pub(super) fn push(&mut self, byte: u8, source: InputSource) -> bool {
        if self.len < self.bytes.len() {
            self.bytes[(self.head + self.len) % self.bytes.len()] = (byte, source);
            self.len += 1;
            return true;
        }
        false
    }

    pub(super) fn drain(&mut self, mut process: impl FnMut(u8, InputSource) -> bool) {
        while self.len != 0 {
            let (byte, source) = self.bytes[self.head];
            if !process(byte, source) {
                break;
            }
            self.head = (self.head + 1) % self.bytes.len();
            self.len -= 1;
        }
    }
}
