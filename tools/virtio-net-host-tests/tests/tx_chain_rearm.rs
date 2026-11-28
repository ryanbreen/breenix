//! A chain that does not fit keeps its completion interrupt (virtio 1.2
//! sections 2.7.7 and 2.7.14). The queue of 16 descriptors holds eight
//! two-descriptor chains when a three-descriptor chain is submitted. As the
//! driver stores its request for an interrupt, the device returns the first
//! chain without seeing the request, so it raises none. The re-check takes
//! that chain back, two descriptors are still too few, and the submission is
//! refused as full. The device's next completion must then interrupt, or the
//! sender waits for traffic that may never come.
use virtio_net_host_tests::drivers::virtio::net_tx_queue::{need_event, TxError, TxQueue, TxRing};

const SIZE: u16 = 16;

/// The rings as the device sees them, with its interrupt decision.
struct Model {
    used: [u32; SIZE as usize],
    used_idx: u16,
    used_event: u16,
    avail_flags: u16,
    event_idx: bool,
    /// Heads offered and not yet returned, oldest first.
    offered: Vec<u16>,
    interrupts: u32,
    /// Return the oldest chain just before the next interrupt request lands.
    race_request: bool,
}

impl Model {
    /// Return the oldest offered chain and interrupt as section 2.7.7.2 says,
    /// from the suppression the device can see now.
    fn complete_one(&mut self) {
        let head = self.offered.remove(0);
        let old = self.used_idx;
        self.used[usize::from(old % SIZE)] = u32::from(head);
        self.used_idx = old.wrapping_add(1);
        let interrupt = if self.event_idx {
            need_event(self.used_event, self.used_idx, old)
        } else {
            self.avail_flags & 1 == 0
        };
        if interrupt {
            self.interrupts += 1;
        }
    }

    fn request_lands(&mut self) {
        if std::mem::take(&mut self.race_request) {
            self.complete_one();
        }
    }
}

impl TxRing for Model {
    fn write_desc(&mut self, _slot: u16, _addr: u64, _len: u32) {}
    fn write_desc_next(&mut self, _slot: u16, _addr: u64, _len: u32, _next: u16) {}
    fn write_avail_entry(&mut self, _pos: u16, slot: u16) {
        self.offered.push(slot);
    }
    fn barrier_before_avail_idx(&mut self) {}
    fn write_avail_idx(&mut self, _idx: u16) {}
    fn barrier_before_notify(&mut self) {}
    fn read_used_idx(&mut self) -> u16 {
        self.used_idx
    }
    fn read_used_id(&mut self, pos: u16) -> u32 {
        self.used[usize::from(pos)]
    }
    fn read_avail_event(&mut self) -> u16 {
        0
    }
    fn read_used_flags(&mut self) -> u16 {
        0
    }
    fn write_used_event(&mut self, idx: u16) {
        if idx == self.used_idx {
            self.request_lands();
        }
        self.used_event = idx;
    }
    fn write_avail_flags(&mut self, flags: u16) {
        if flags == 0 {
            self.request_lands();
        }
        self.avail_flags = flags;
    }
    fn full_barrier(&mut self) {}
}

fn scenario(event_idx: bool) {
    let model = Model {
        used: [0; SIZE as usize],
        used_idx: 0,
        used_event: 0,
        avail_flags: 0,
        event_idx,
        offered: Vec::new(),
        interrupts: 0,
        race_request: false,
    };
    let mut q = TxQueue::new(model);
    q.configure(SIZE, SIZE);
    q.start(event_idx);
    for _ in 0..8 {
        q.submit_chain(2, |slots, out| {
            for (s, o) in slots.iter().zip(out.iter_mut()) {
                *o = (u64::from(*s) * 2048, 100);
            }
        })
        .expect("room for a two-descriptor chain");
    }
    assert_eq!(q.in_flight(), 16);

    q.ring_mut().race_request = true;
    let refused = q.submit_chain(3, |_, _| panic!("no room was expected"));
    assert_eq!(refused, Err(TxError::Full), "event_idx {event_idx}");
    assert_eq!(q.in_flight(), 14, "event_idx {event_idx}: the raced chain was taken back");
    assert_eq!(q.ring_mut().interrupts, 0, "event_idx {event_idx}: the raced completion saw no request");

    q.ring_mut().complete_one();
    assert_eq!(
        q.ring_mut().interrupts,
        1,
        "event_idx {event_idx}: the completion after a refused submission did not interrupt"
    );
}

#[test]
fn a_refused_chain_keeps_its_completion_interrupt_with_event_idx() {
    scenario(true);
}

#[test]
fn a_refused_chain_keeps_its_completion_interrupt_with_no_interrupt_flag() {
    scenario(false);
}
