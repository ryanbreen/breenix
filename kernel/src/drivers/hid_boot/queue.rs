//! Keyboard bytes retained until the console TTY accepts them.
//!
//! `tty::push_char_nonblock` refuses a byte while the console or its line
//! discipline is locked. A refused byte stays at the head of this queue and is
//! offered again, so a busy lock delays a keystroke rather than dropping it. A
//! key event is queued whole or refused whole, so an escape sequence stays in
//! one piece. Pure and allocation-free: `rustc --edition=2021 --test queue.rs`
//! runs its tests.

pub struct Retained<const N: usize> {
    bytes: [u8; N],
    head: usize,
    len: usize,
    refused: u64,
}

impl<const N: usize> Retained<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            head: 0,
            len: 0,
            refused: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Key events refused because the queue had no room for their bytes.
    pub fn refused(&self) -> u64 {
        self.refused
    }

    /// Queue one key event's bytes whole, or refuse the event.
    pub fn push(&mut self, event: &[u8]) -> bool {
        if event.len() > N - self.len {
            self.refused += 1;
            return false;
        }
        for &byte in event {
            self.bytes[(self.head + self.len) % N] = byte;
            self.len += 1;
        }
        true
    }

    /// Offer bytes in order until `accept` refuses one, which stays queued.
    /// Returns how many were accepted.
    pub fn drain(&mut self, accept: &mut dyn FnMut(u8) -> bool) -> usize {
        let mut taken = 0;
        while self.len != 0 && accept(self.bytes[self.head]) {
            self.head = (self.head + 1) % N;
            self.len -= 1;
            taken += 1;
        }
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_busy_sink_loses_no_byte_and_keeps_order() {
        let mut q = Retained::<16>::new();
        assert!(q.push(b"ab") && q.push(b"\x1b[A") && q.push(b"c"));
        let mut seen = Vec::new();
        // The sink refuses each third offer, as a held lock would.
        let mut offers = 0;
        while !q.is_empty() {
            q.drain(&mut |b| {
                offers += 1;
                if offers % 3 == 0 {
                    return false;
                }
                seen.push(b);
                true
            });
        }
        assert_eq!(seen, b"ab\x1b[Ac");
        assert_eq!(q.refused(), 0);
    }

    #[test]
    fn a_refusing_sink_keeps_everything_queued() {
        let mut q = Retained::<8>::new();
        q.push(b"xyz");
        assert_eq!(q.drain(&mut |_| false), 0);
        assert_eq!(q.len(), 3);
        let mut seen = Vec::new();
        assert_eq!(q.drain(&mut |b| { seen.push(b); true }), 3);
        assert_eq!(seen, b"xyz");
    }

    #[test]
    fn events_are_whole_and_the_ring_wraps() {
        let mut q = Retained::<5>::new();
        assert!(q.push(b"abc"));
        // Four more bytes do not fit beside three: the event is refused whole.
        assert!(!q.push(b"\x1b[3~"));
        assert_eq!((q.len(), q.refused()), (3, 1));
        let mut seen = Vec::new();
        q.drain(&mut |b| if seen.len() < 2 { seen.push(b); true } else { false });
        assert_eq!(seen, b"ab");
        assert_eq!(q.len(), 1);
        // Head is now at index 2; this event wraps past the end.
        assert!(q.push(b"\x1b[3~"));
        seen.clear();
        q.drain(&mut |b| { seen.push(b); true });
        assert_eq!(seen, b"c\x1b[3~");
        assert!(q.is_empty());
    }
}
