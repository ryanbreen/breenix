//! The transmit queue's publication, as the device sees it (virtio 1.2 section
//! 2.7.13): `avail.idx` only moves forward, one buffer at a time, and only past
//! a ring entry the driver wrote and ordered before the index with a barrier.
//!
//! The two-CPU model runs one CPU's submission through the production
//! `TxQueue::submit` and stops it after each of its stores to queue memory to
//! let the second CPU try to submit through the same lock. Behind
//! `spin::Mutex`, the lock type the kernel's `IrqSafeMutex` wraps, the second
//! CPU must find the lock held at each of those six points and go only after
//! the first CPU's index store.

use crate::net_tx_queue::{TxError, TxQueue, TxRing};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU16, Ordering};

const RING: u16 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Store {
    Desc(u16),
    Entry { pos: u16, slot: u16 },
    Barrier,
    Idx(u16),
    NotifyBarrier,
}

#[derive(Default)]
struct Device {
    stores: Vec<Store>,
    used_idx: u16,
    used: [u32; RING as usize],
}

impl Device {
    /// The device completes `slot` by writing it to the next used entry.
    fn complete(&mut self, size: u16, slot: u32) {
        self.used[(self.used_idx % size) as usize] = slot;
        self.used_idx = self.used_idx.wrapping_add(1);
    }
}

type Hook = Rc<RefCell<Option<Box<dyn FnMut()>>>>;

struct TestRing {
    dev: Rc<RefCell<Device>>,
    hook: Hook,
    steps: Rc<Cell<usize>>,
    hook_at: usize,
}

impl TestRing {
    fn new(dev: &Rc<RefCell<Device>>) -> Self {
        Self {
            dev: dev.clone(),
            hook: Rc::new(RefCell::new(None)),
            steps: Rc::new(Cell::new(0)),
            hook_at: 0,
        }
    }

    /// Count one access to queue memory; the other CPU runs before the
    /// `hook_at`-th one.
    fn step(&mut self) {
        let n = self.steps.get() + 1;
        self.steps.set(n);
        if n == self.hook_at {
            let hook = self.hook.borrow_mut().take();
            if let Some(mut hook) = hook {
                hook();
            }
        }
    }

    fn store(&mut self, store: Store) {
        self.step();
        self.dev.borrow_mut().stores.push(store);
    }
}

impl TxRing for TestRing {
    fn write_desc(&mut self, slot: u16, _addr: u64, _len: u32) {
        self.store(Store::Desc(slot));
    }
    fn write_avail_entry(&mut self, pos: u16, slot: u16) {
        self.store(Store::Entry { pos, slot });
    }
    fn barrier_before_avail_idx(&mut self) {
        self.store(Store::Barrier);
    }
    fn write_avail_idx(&mut self, idx: u16) {
        self.store(Store::Idx(idx));
    }
    fn barrier_before_notify(&mut self) {
        self.store(Store::NotifyBarrier);
    }
    fn read_used_idx(&mut self) -> u16 {
        self.step();
        self.dev.borrow().used_idx
    }
    fn read_used_id(&mut self, pos: u16) -> u32 {
        self.step();
        self.dev.borrow().used[pos as usize]
    }
}

/// Replay the stores in the order the device can observe them and return the
/// descriptors it was offered, or the first store that breaks section 2.7.13.
fn device_view(stores: &[Store], size: u16) -> Result<Vec<u16>, String> {
    let mut idx = 0u16;
    let mut entry: Option<(u16, u16, bool)> = None;
    let mut offered = Vec::new();
    for &store in stores {
        match store {
            Store::Entry { pos, slot } => entry = Some((pos, slot, false)),
            Store::Barrier => {
                if let Some(e) = entry.as_mut() {
                    e.2 = true;
                }
            }
            Store::Idx(new) => {
                if new != idx.wrapping_add(1) {
                    return Err(format!("avail.idx moved from {idx} to {new}"));
                }
                match entry.take() {
                    Some((pos, slot, true)) if pos == idx % size => offered.push(slot),
                    Some((pos, _, true)) => {
                        return Err(format!("avail.idx {new} covers position {}, but entry {pos} was written", idx % size))
                    }
                    Some((_, _, false)) => return Err(format!("avail.idx {new} stored with no barrier after its entry")),
                    None => return Err(format!("avail.idx {new} stored with no entry written")),
                }
                idx = new;
            }
            Store::Desc(_) | Store::NotifyBarrier => {}
        }
    }
    Ok(offered)
}

fn data(slot: u16) -> (u64, u32) {
    (0x1000 * u64::from(slot), 60)
}

#[test]
fn a_second_cpu_cannot_publish_inside_the_first_cpus_submission() {
    // A submission with no completions to reclaim makes six accesses: used.idx, the
    // descriptor, the ring entry, the barrier, avail.idx and the notify barrier.
    for hook_at in 1..=6 {
        let dev = Rc::new(RefCell::new(Device::default()));
        let mut ring = TestRing::new(&dev);
        ring.hook_at = hook_at;
        let hook = ring.hook.clone();
        let steps = ring.steps.clone();
        let queue: &'static spin::Mutex<TxQueue<TestRing>> =
            Box::leak(Box::new(spin::Mutex::new(TxQueue::new(ring))));
        queue.lock().configure(RING, RING);

        let blocked = Rc::new(Cell::new(false));
        let b_blocked = blocked.clone();
        *hook.borrow_mut() = Some(Box::new(move || match queue.try_lock() {
            Some(mut q) => {
                q.submit(data).expect("second CPU submits");
            }
            None => b_blocked.set(true),
        }));

        let a = queue.lock().submit(data);
        assert_eq!(a, Ok(1), "injection at access {hook_at}");
        assert_eq!(steps.get(), 6, "the submission made six accesses");
        assert!(blocked.get(), "the second CPU got in at access {hook_at}");
        // It spun on the lock and goes now.
        assert_eq!(queue.lock().submit(data), Ok(2));

        let stores = dev.borrow().stores.clone();
        assert_eq!(device_view(&stores, RING), Ok(vec![0, 1]), "access {hook_at}: {stores:?}");
    }
}

#[test]
fn the_check_catches_the_unserialized_reservation_it_replaced() {
    // The order the driver used before this check: the index was reserved with
    // a shared fetch_add, and the entry and index were stored after it with no
    // lock, so a second CPU could reserve, write and publish in between.
    let dev = RefCell::new(Device::default());
    let head = AtomicU16::new(0);
    let transmit = |slot: u16, between: &dyn Fn()| {
        let avail_idx = head.fetch_add(1, Ordering::AcqRel);
        between();
        let mut d = dev.borrow_mut();
        d.stores.push(Store::Desc(slot));
        d.stores.push(Store::Entry { pos: avail_idx % RING, slot });
        d.stores.push(Store::Barrier);
        d.stores.push(Store::Idx(avail_idx.wrapping_add(1)));
    };
    transmit(0, &|| transmit(1, &|| {}));
    let stores = dev.borrow().stores.clone();
    assert_eq!(device_view(&stores, RING), Err("avail.idx moved from 0 to 2".to_string()));
}

#[test]
fn a_full_queue_is_reclaimed_by_the_next_transmit_at_the_devices_queue_size() {
    let dev = Rc::new(RefCell::new(Device::default()));
    let mut queue = TxQueue::new(TestRing::new(&dev));
    // The device offers 8 descriptors; the static ring has room for 16.
    queue.configure(8, RING);
    assert_eq!(queue.size(), 8);
    for n in 1..=8 {
        assert_eq!(queue.submit(data), Ok(n));
    }
    assert_eq!(queue.submit(data), Err(TxError::Full));

    // A used entry for a descriptor the driver did not hand over leaves the queue full.
    dev.borrow_mut().complete(8, 31);
    assert_eq!(queue.submit(data), Err(TxError::Full));

    // The device finishes descriptor 5; the transmit path takes it back itself.
    dev.borrow_mut().complete(8, 5);
    assert_eq!(queue.submit(data), Ok(9));
    assert_eq!(queue.in_flight(), 8);
    let stores = dev.borrow().stores.clone();
    let offered = device_view(&stores, 8).expect("publication order");
    assert_eq!(offered, vec![0, 1, 2, 3, 4, 5, 6, 7, 5]);
    assert!(stores.contains(&Store::Entry { pos: 0, slot: 5 }), "position 8 wraps to 0 in an 8-entry ring");
}
