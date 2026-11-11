use libcollab::queue::{OutputQueues, SendError, Transport, QUEUE_LIMIT, SYNC_LOW_WATER};
use libcollab::wire::{decode, Message};
use std::collections::BTreeMap;

#[derive(Default)]
struct Fake {
    received: BTreeMap<u32, Vec<u8>>,
    chunk: usize,
    blocked: usize,
}
impl Transport for Fake {
    type Error = ();
    fn send(&mut self, conn: u32, bytes: &[u8]) -> Result<usize, SendError<Self::Error>> {
        if self.blocked > 0 {
            self.blocked -= 1;
            return Err(SendError::WouldBlock);
        }
        let n = bytes.len().min(self.chunk);
        self.received
            .entry(conn)
            .or_default()
            .extend_from_slice(&bytes[..n]);
        Ok(n)
    }
}

#[test]
fn partial_writes_and_backpressure_preserve_order() {
    let mut q = OutputQueues::new();
    q.connect(1);
    q.connect(2);
    q.enqueue(1, b"first").unwrap();
    q.enqueue(1, b"second").unwrap();
    q.enqueue(2, b"other").unwrap();
    let mut t = Fake {
        chunk: 2,
        blocked: 3,
        ..Fake::default()
    };
    for _ in 0..3 {
        q.flush(1, &mut t).unwrap();
        assert_eq!(q.queued(1), 11);
    }
    while q.queued(1) != 0 {
        q.flush(1, &mut t).unwrap();
    }
    while q.queued(2) != 0 {
        q.flush(2, &mut t).unwrap();
    }
    assert_eq!(t.received[&1], b"firstsecond");
    assert_eq!(t.received[&2], b"other");
    assert!(!q.wants_write(1));
}

#[test]
fn slow_reader_is_closed_without_affecting_other_connections() {
    let mut q = OutputQueues::new();
    q.connect(1);
    q.connect(2);
    q.enqueue(1, &vec![7; QUEUE_LIMIT]).unwrap();
    assert_eq!(q.enqueue(1, b"x"), Err(b"queue-full".as_slice()));
    assert_eq!(q.close_reason(1), Some(b"queue-full".as_slice()));
    q.enqueue(2, b"still alive").unwrap();
    let mut t = Fake {
        chunk: 100,
        ..Fake::default()
    };
    q.flush(2, &mut t).unwrap();
    assert_eq!(t.received[&2], b"still alive");
}

#[test]
fn canvas_sync_is_generated_on_demand_below_low_water() {
    let canvas: Vec<u8> = (0..800 * 600 * 4).map(|i| i as u8).collect();
    let mut q = OutputQueues::new();
    q.connect(1);
    q.start_sync(1, canvas.clone()).unwrap();
    let mut t = Fake {
        chunk: 4096,
        ..Fake::default()
    };
    while q.syncing(1) || q.wants_write(1) {
        q.fill_sync(1).unwrap();
        assert!(q.queued(1) <= SYNC_LOW_WATER + 65540);
        assert!(q.queued(1) < QUEUE_LIMIT);
        q.flush(1, &mut t).unwrap();
    }
    let mut remaining = &t.received[&1][..];
    let mut rebuilt = Vec::new();
    let mut ended = false;
    while !remaining.is_empty() {
        let (msg, used) = decode(remaining).unwrap().unwrap();
        remaining = &remaining[used..];
        match msg {
            Message::SyncChunk { offset, data } => {
                assert_eq!(offset as usize, rebuilt.len());
                rebuilt.extend(data);
            }
            Message::SyncEnd => ended = true,
            _ => panic!("unexpected frame"),
        }
    }
    assert!(ended);
    assert_eq!(rebuilt, canvas);
}
