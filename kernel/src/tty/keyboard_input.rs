//! Fixed storage for keyboard bytes awaiting the terminal's line discipline.
//! A busy discipline keeps the oldest byte queued; no byte reaches a reader raw.
pub(super) struct PendingInput {
    bytes: [u8; 256],
    head: usize,
    len: usize,
}

impl PendingInput {
    pub(super) const fn new() -> Self {
        Self {
            bytes: [0; 256],
            head: 0,
            len: 0,
        }
    }

    pub(super) fn push(&mut self, byte: u8) {
        if self.len < self.bytes.len() {
            self.bytes[(self.head + self.len) % self.bytes.len()] = byte;
            self.len += 1;
        }
    }

    pub(super) fn drain(&mut self, mut process: impl FnMut(u8) -> bool) {
        while self.len != 0 {
            if !process(self.bytes[self.head]) {
                break;
            }
            self.head = (self.head + 1) % self.bytes.len();
            self.len -= 1;
        }
    }
}
