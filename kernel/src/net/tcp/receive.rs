//! Out-of-order payload and FIN state for the TCP receive half.
use super::seq_lt;
use alloc::vec::Vec;

// Payload is bounded by the advertised sequence interval. Cap metadata
// separately so a window full of one-byte segments cannot grow without bound.
const MAX_SPANS: usize = 256;
const SACK_BLOCKS: usize = 4;
const SACK_KIND: u8 = 5;

pub(super) struct Span {
    pub seq: u32,
    pub bytes: Vec<u8>,
    arrival: u64,
}

impl Span {
    fn end(&self) -> u32 {
        self.seq.wrapping_add(self.bytes.len() as u32)
    }
}

#[derive(Default)]
pub(super) struct Pending {
    // In sequence order, disjoint, and within one receive window of RCV.NXT.
    spans: Vec<Span>,
    arrival: u64,
    pub fin: Option<u32>,
}

impl Pending {
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty() && self.fin.is_none()
    }

    pub fn next_seq(&self) -> Option<u32> {
        self.spans.first().map(|span| span.seq)
    }

    pub fn take_front(&mut self) -> Span {
        self.spans.remove(0)
    }

    /// Put back the unread suffix when publishing to the stream ran out of memory.
    pub fn restore_front(&mut self, mut span: Span, taken: usize) {
        span.bytes.drain(..taken);
        span.seq = span.seq.wrapping_add(taken as u32);
        self.spans.insert(0, span);
    }

    /// Save only bytes not already covered by a span. Leave the older copy in
    /// place across overlaps, and remember which block caused the current ACK.
    pub fn insert(&mut self, mut seq: u32, mut payload: &[u8]) {
        self.arrival = self.arrival.saturating_add(1);
        let mut index = self.spans.partition_point(|span| !seq_lt(seq, span.end()));
        while !payload.is_empty() {
            let uncovered = match self.spans.get_mut(index) {
                Some(span) if !seq_lt(seq, span.seq) => {
                    span.arrival = self.arrival;
                    let skip = payload.len().min(span.end().wrapping_sub(seq) as usize);
                    seq = seq.wrapping_add(skip as u32);
                    payload = &payload[skip..];
                    index += 1;
                    continue;
                }
                Some(span) => payload.len().min(span.seq.wrapping_sub(seq) as usize),
                None => payload.len(),
            };
            if self.spans.len() == MAX_SPANS || self.spans.try_reserve(1).is_err() {
                break;
            }
            let mut bytes = Vec::new();
            if bytes.try_reserve_exact(uncovered).is_err() {
                break;
            }
            bytes.extend_from_slice(&payload[..uncovered]);
            self.spans.insert(
                index,
                Span {
                    seq,
                    bytes,
                    arrival: self.arrival,
                },
            );
            seq = seq.wrapping_add(uncovered as u32);
            payload = &payload[uncovered..];
            index += 1;
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// RFC 2018 §4. Join adjacent spans for the wire and report the newest
    /// contiguous blocks first, including the block that elicited this ACK.
    pub fn sack_option<'a>(&self, encoded: &'a mut [u8; 36]) -> &'a [u8] {
        #[derive(Copy, Clone, Default)]
        struct Block {
            start: u32,
            end: u32,
            arrival: u64,
        }
        let mut blocks = [Block::default(); SACK_BLOCKS];
        let mut spans = self.spans.iter().peekable();
        while let Some(span) = spans.next() {
            let mut block = Block {
                start: span.seq,
                end: span.end(),
                arrival: span.arrival,
            };
            while let Some(next) = spans.next_if(|next| next.seq == block.end) {
                block.end = next.end();
                block.arrival = block.arrival.max(next.arrival);
            }
            if let Some(place) = blocks
                .iter()
                .position(|other| other.arrival <= block.arrival)
            {
                blocks[place..].rotate_right(1);
                blocks[place] = block;
            }
        }
        let count = blocks.iter().take_while(|block| block.arrival != 0).count();
        if count == 0 {
            return &[];
        }
        let end = 2 + count * 8;
        encoded[0] = SACK_KIND;
        encoded[1] = end as u8;
        for (option, block) in encoded[2..end].chunks_exact_mut(8).zip(blocks) {
            option[..4].copy_from_slice(&block.start.to_be_bytes());
            option[4..].copy_from_slice(&block.end.to_be_bytes());
        }
        &encoded[..end + 2] // Two zero bytes end the option list and align it.
    }
}
