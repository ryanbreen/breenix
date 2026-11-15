use crate::block::{BlockDevice, BlockError};
use crate::buffer_cache::{BufferCache, BufferStatus, WriteGroup};
use crate::recording::{Event, HostWait, Op, RecordingDevice, SIZE};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

fn spawn(name: &str, work: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(work)
        .unwrap()
}

#[test]
fn flush_orders_later_writes() {
    for hold in [false, true] {
        flush_ordering(hold);
    }
}

fn flush_ordering(hold: bool) {
    let device = Arc::new(RecordingDevice::default());
    let cache = Arc::new(BufferCache::new(HostWait::new(device.clone())));
    let group = WriteGroup::hold(&*device, Arc::new(|| {}));
    if hold {
        cache.write(&*device, group.clone(), 1, &[1; SIZE]).unwrap();
        cache.write(&*device, group.clone(), 3, &[3; SIZE]).unwrap();
    }
    device.hold(true, true);
    let earlier = {
        let (c, d) = (cache.clone(), device.clone());
        spawn("earlier", move || {
            if hold {
                c.submit(&*d, &group).wait().unwrap();
            } else {
                c.write(&*d, WriteGroup::WriteThrough, 1, &[1; SIZE])
                    .unwrap();
            }
        })
    };
    device.until(|e| e.contains(&Event::Issue(Op::Write(1))));
    let flush = {
        let (c, d) = (cache.clone(), device.clone());
        spawn("flusher", move || c.flush(&*d).unwrap())
    };
    // Either a correct cache is waiting for the earlier write, or the unordered
    // implementation has already sent the device flush. Neither path uses sleeps.
    device.until(|e| {
        e.contains(&Event::Waiting("flusher".into())) || e.contains(&Event::Issue(Op::Flush))
    });
    let later = {
        let (c, d) = (cache.clone(), device.clone());
        spawn("later", move || {
            c.write(&*d, WriteGroup::WriteThrough, 2, &[2; SIZE])
                .unwrap()
        })
    };
    device.until(|e| {
        e.contains(&Event::Waiting("later".into())) || e.contains(&Event::Issue(Op::Write(2)))
    });
    let while_draining = device.events();
    device.hold(false, true);
    device.until(|e| e.contains(&Event::Issue(Op::Flush)));
    let while_flushing = device.events();
    // Release and join before asserting, including on the unordered baseline.
    device.hold(false, false);
    earlier.join().unwrap();
    flush.join().unwrap();
    later.join().unwrap();
    assert!(
        !while_draining.contains(&Event::Issue(Op::Flush)),
        "flush overtook an issued write: {while_draining:?}"
    );
    assert!(
        !while_draining.contains(&Event::Issue(Op::Write(2))),
        "later write passed a draining flush: {while_draining:?}"
    );
    assert!(
        !while_flushing.contains(&Event::Issue(Op::Write(2))),
        "later write passed an open device flush: {while_flushing:?}"
    );
    let events = device.events();
    let at = |event| events.iter().position(|e| *e == event).unwrap();
    assert!(at(Event::Complete(Op::Write(1))) < at(Event::Issue(Op::Flush)));
    if hold {
        assert!(at(Event::Complete(Op::Write(3))) < at(Event::Issue(Op::Flush)));
    }
    assert!(at(Event::Complete(Op::Flush)) < at(Event::Issue(Op::Write(2))));
}

#[test]
fn write_through_completes_before_return() {
    let device = Arc::new(RecordingDevice::default());
    let cache = Arc::new(BufferCache::new(HostWait::new(device.clone())));
    device.hold(true, false);
    let writer = {
        let (c, d) = (cache.clone(), device.clone());
        spawn("writer", move || {
            c.write(&*d, WriteGroup::WriteThrough, 1, &[0x5a; SIZE])
                .unwrap();
            d.record(Event::Returned(Op::Write(1)));
        })
    };
    device.until(|e| {
        e.contains(&Event::Issue(Op::Write(1))) || e.contains(&Event::Returned(Op::Write(1)))
    });
    let held = device.events();
    device.hold(false, false);
    writer.join().unwrap();
    assert!(
        !held.contains(&Event::Returned(Op::Write(1))),
        "write returned with device operation open: {held:?}"
    );
    assert_eq!(
        device.events(),
        vec![
            Event::Issue(Op::Write(1)),
            Event::Complete(Op::Write(1)),
            Event::Returned(Op::Write(1)),
        ]
    );
    let mut bytes = [0; SIZE];
    device.read_block(1, &mut bytes).unwrap();
    assert_eq!(bytes, [0x5a; SIZE]);
}

#[test]
fn write_counters_stamp_on_completion() {
    for hold in [false, true] {
        for fail in [false, true] {
            let device = Arc::new(RecordingDevice::default());
            let cache = Arc::new(BufferCache::new(HostWait::new(device.clone())));
            let group = WriteGroup::hold(&*device, Arc::new(|| {}));
            if hold {
                cache.write(&*device, group.clone(), 1, &[1; SIZE]).unwrap();
            }
            let before = cache.counters();
            device.fail_write(if fail { Some(1) } else { None });
            device.hold(true, false);
            let writer = {
                let (c, d) = (cache.clone(), device.clone());
                spawn("counter-writer", move || {
                    let result = if hold {
                        c.submit(&*d, &group).wait()
                    } else {
                        c.write(&*d, WriteGroup::WriteThrough, 1, &[1; SIZE])
                    };
                    assert_eq!(result, if fail { Err(BlockError::IoError) } else { Ok(()) });
                })
            };
            device.until(|e| e.contains(&Event::Issue(Op::Write(1))));
            let held = cache.counters();
            // Read the public report so this check also runs on the pre-gauge code.
            let pending = crate::cache_report::render(&held)
                .lines()
                .find_map(|line| {
                    let mut fields = line.split_whitespace();
                    (fields.next() == Some("pending_writes")).then(|| {
                        (fields.next().unwrap().parse::<u64>().unwrap(),
                         fields.next().unwrap().parse::<u64>().unwrap())
                    })
                });
            device.hold(false, false);
            writer.join().unwrap();
            let after = cache.counters();
            assert_eq!(held.device_writes, before.device_writes,
                "counted an open write: hold={hold}, fail={fail}");
            let (pending_count, raised_sequence) = pending.expect("pending_writes gauge");
            assert_eq!(pending_count, 1);
            assert_eq!(after.device_writes.count, before.device_writes.count + 1);
            assert!(after.device_writes.sequence > raised_sequence);
            let report = crate::cache_report::render(&after);
            let pending: Vec<_> = report.lines()
                .find(|line| line.starts_with("pending_writes "))
                .unwrap().split_whitespace().collect();
            assert_eq!(pending[1], "0");
            assert!(pending[2].parse::<u64>().unwrap() > raised_sequence);
        }
    }
}

#[test]
fn lru_evicts_within_bound() {
    let device = Arc::new(RecordingDevice::default());
    let cache = BufferCache::with_capacity(HostWait::new(device.clone()), 3 * SIZE);
    let mut bytes = [0; SIZE];
    for block in [1, 2, 3] {
        cache.read(&*device, block, &mut bytes).unwrap();
        assert!(cache.resident_bytes() <= 3 * SIZE);
    }
    let pinned = cache.pin(&*device, 1).expect("pin resident block");
    cache.read(&*device, 2, &mut bytes).unwrap(); // 3 is LRU, 1 is pinned.
    cache.read(&*device, 4, &mut bytes).unwrap();
    assert_eq!(cache.resident_bytes(), 3 * SIZE);
    let before = cache.counters();
    cache.read(&*device, 1, &mut bytes).unwrap();
    cache.read(&*device, 2, &mut bytes).unwrap();
    assert_eq!(cache.counters().device_reads, before.device_reads);
    cache.read(&*device, 3, &mut bytes).unwrap();
    assert_eq!(
        cache.counters().device_reads.count,
        before.device_reads.count + 1
    );
    assert_eq!(&pinned[..], &[0; SIZE]);
    // Retained snapshots still count after a write replaces their cache key.
    cache
        .write(&*device, WriteGroup::WriteThrough, 1, &[9; SIZE])
        .unwrap();
    assert_eq!(&pinned[..], &[0; SIZE]);
    for block in 5..12 {
        cache.read(&*device, block, &mut bytes).unwrap();
        assert!(cache.resident_bytes() <= 3 * SIZE);
    }
    drop(pinned);
    assert!(cache.resident_bytes() <= 3 * SIZE);
}

#[test]
fn bound_is_an_eighth_clamped() {
    use crate::buffer_cache::bound_for_memory;
    const MIB: usize = 1024 * 1024;
    for (memory, expected) in [
        (0, 4 * MIB),
        (32 * MIB - 1, 4 * MIB),
        (32 * MIB, 4 * MIB),
        (64 * MIB, 8 * MIB),
        (8 * 1024 * MIB, 1024 * MIB),
        (usize::MAX, 1024 * MIB),
    ] {
        assert_eq!(bound_for_memory(memory), expected, "memory={memory}");
    }
}

#[test]
fn failed_verify_returns_eio_and_caches_nothing() {
    use crate::block::BlockError;
    let device = Arc::new(RecordingDevice::default());
    device.write_block(1, &[0x5a; SIZE]).unwrap();
    let cache = BufferCache::new(HostWait::new(device.clone()));
    let mut bytes = [0; SIZE];
    let calls = std::cell::Cell::new(0);
    let result = cache.read_verified(&*device, 1, &mut bytes, |data| {
        calls.set(calls.get() + 1);
        assert_eq!(data, &[0x5a; SIZE]);
        false
    });
    assert_eq!(result, Err(BlockError::IoError));
    assert_eq!(calls.get(), 1);
    assert_eq!(cache.resident_bytes(), 0);
    assert!(cache.pin(&*device, 1).is_none());
    let refused = cache.counters();
    assert_eq!(refused.misses.count, 1);
    assert_eq!(refused.device_reads.count, 1);
    assert!(refused.misses.sequence < refused.device_reads.sequence);
    cache
        .read_verified(&*device, 1, &mut bytes, |_| {
            calls.set(calls.get() + 1);
            true
        })
        .unwrap();
    assert_eq!(bytes, [0x5a; SIZE]);
    assert_eq!(calls.get(), 2);
    let accepted = cache.counters();
    assert_eq!(accepted.misses.count, 2);
    assert_eq!(accepted.device_reads.count, 2);
    assert!(refused.device_reads.sequence < accepted.misses.sequence);
    assert!(accepted.misses.sequence < accepted.device_reads.sequence);
    cache
        .read_verified(&*device, 1, &mut bytes, |_| panic!("verify ran on hit"))
        .unwrap();
    let hit = cache.counters();
    assert_eq!(hit.hits.count, 1);
    assert_eq!(hit.device_reads, accepted.device_reads);
    assert!(accepted.device_reads.sequence < hit.hits.sequence);
}

#[test]
fn reader_waits_for_inflight_miss() {
    let device = Arc::new(RecordingDevice::default());
    device.write_block(1, &[0xa5; SIZE]).unwrap();
    let cache = Arc::new(BufferCache::with_capacity(
        HostWait::new(device.clone()),
        SIZE,
    ));
    device.hold_reads(true);
    let first = {
        let (c, d) = (cache.clone(), device.clone());
        spawn("first-reader", move || {
            let mut bytes = [0; SIZE];
            c.read(&*d, 1, &mut bytes).unwrap();
            assert_eq!(bytes, [0xa5; SIZE]);
        })
    };
    device.until(|e| e.contains(&Event::Issue(Op::Read(1))));
    device.hold_wait_return(true);
    let second = {
        let (c, d) = (cache.clone(), device.clone());
        spawn("second-reader", move || {
            let mut bytes = [0; SIZE];
            c.read(&*d, 1, &mut bytes).unwrap();
            assert_eq!(bytes, [0xa5; SIZE]);
            d.record(Event::Returned(Op::Read(1)));
        })
    };
    device.until(|e| {
        e.contains(&Event::Waiting("second-reader".into()))
            || e.contains(&Event::Returned(Op::Read(1)))
    });
    let held = device.events();
    device.hold_reads(false);
    first.join().unwrap();
    device.until(|e| e.contains(&Event::WaitReturning("second-reader".into())));
    // Force another device to seek the entire budget while the awakened reader
    // has not taken its FIFO turn. Its first read's bytes must remain pinned.
    let pressure = RecordingDevice::default();
    let mut scratch = [0; SIZE];
    cache.read(&pressure, 2, &mut scratch).unwrap();
    let retained = cache.pin(&*device, 1).is_some();
    let resident = cache.resident_bytes();
    device.hold_wait_return(false);
    second.join().unwrap();
    assert!(retained, "evicted bytes belonging to an awakened reader");
    assert!(resident <= SIZE);
    assert!(!held.contains(&Event::Returned(Op::Read(1))));
    assert_eq!(
        device
            .events()
            .iter()
            .filter(|e| **e == Event::Issue(Op::Read(1)))
            .count(),
        1
    );
    let counters = cache.counters();
    assert_eq!(counters.misses.count, 2);
    assert_eq!(counters.hits.count, 1);
    assert_eq!(counters.device_reads.count, 2);
    assert!(counters.misses.sequence < counters.device_reads.sequence);
    assert!(counters.device_reads.sequence < counters.hits.sequence);
}

#[test]
fn hold_writes_nothing_until_submit() {
    let device = Arc::new(RecordingDevice::default());
    let cache = BufferCache::new(HostWait::new(device.clone()));
    let group = WriteGroup::hold(&*device, Arc::new(|| {}));
    let other = WriteGroup::hold(&*device, Arc::new(|| {}));
    cache.write(&*device, group.clone(), 1, &[1; SIZE]).unwrap();
    cache.write(&*device, group.clone(), 2, &[2; SIZE]).unwrap();
    cache.write(&*device, other.clone(), 3, &[3; SIZE]).unwrap();
    // A second dirty version replaces only this group's earlier bytes.
    cache.write(&*device, group.clone(), 2, &[9; SIZE]).unwrap();
    assert_eq!(cache.status(&*device, 2), Some(BufferStatus::Dirty));
    let mut bytes = [0; SIZE];
    cache.read(&*device, 2, &mut bytes).unwrap();
    assert_eq!(bytes, [9; SIZE]);
    cache.flush(&*device).unwrap();
    assert!(!device
        .events()
        .iter()
        .any(|e| matches!(e, Event::Issue(Op::Write(_)))));
    cache.submit(&*device, &group).wait().unwrap();
    for block in [1, 2] {
        assert_eq!(cache.status(&*device, block), Some(BufferStatus::Clean));
    }
    assert_eq!(cache.status(&*device, 3), Some(BufferStatus::Dirty));
    assert!(!device.events().contains(&Event::Issue(Op::Write(3))));
    device.read_block(2, &mut bytes).unwrap();
    assert_eq!(bytes, [9; SIZE]);
    cache.submit(&*device, &other).wait().unwrap();
    assert!(device.events().contains(&Event::Complete(Op::Write(3))));
}

#[test]
fn batch_wait_returns_after_device_writes() {
    for fail in [false, true] {
        let device = Arc::new(RecordingDevice::default());
        let cache = Arc::new(BufferCache::new(HostWait::new(device.clone())));
        let group = WriteGroup::hold(&*device, Arc::new(|| {}));
        for block in [0, 1, 2] {
            cache
                .write(&*device, group.clone(), block, &[block as u8; SIZE])
                .unwrap();
        }
        device.fail_write(if fail { Some(1) } else { None });
        device.hold(true, false);
        let submitter = {
            let (c, d) = (cache.clone(), device.clone());
            spawn("batch", move || {
                let result = c.submit(&*d, &group).wait();
                d.record(Event::BatchReturned(result));
            })
        };
        device.until(|e| {
            e.contains(&Event::Issue(Op::Write(1)))
                || e.iter().any(|e| matches!(e, Event::BatchReturned(_)))
        });
        let held = device.events();
        let inflight = cache.status(&*device, 1);
        device.hold(false, false);
        submitter.join().unwrap();
        assert!(
            !held.iter().any(|e| matches!(e, Event::BatchReturned(_))),
            "batch returned before its held write: {held:?}"
        );
        assert_eq!(inflight, Some(BufferStatus::InFlight));
        let events = device.events();
        let returned = events
            .iter()
            .position(|e| matches!(e, Event::BatchReturned(_)))
            .unwrap();
        for block in [0, 1, 2] {
            let done = if fail && block == 1 {
                Event::Failed(Op::Write(block))
            } else {
                Event::Complete(Op::Write(block))
            };
            assert!(events.iter().position(|e| *e == done).unwrap() < returned);
            assert_eq!(
                cache.status(&*device, block),
                Some(if fail && block == 1 {
                    BufferStatus::Failed
                } else {
                    BufferStatus::Clean
                })
            );
        }
        assert_eq!(
            events[returned],
            Event::BatchReturned(if fail {
                Err(BlockError::IoError)
            } else {
                Ok(())
            })
        );
        if fail {
            assert!(cache.pin(&*device, 1).is_none());
            assert_eq!(
                cache.read(&*device, 1, &mut [0; SIZE]),
                Err(BlockError::IoError)
            );
        }
    }
}

#[test]
fn pressure_asks_holder_to_commit() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let device = Arc::new(RecordingDevice::default());
    let cache = Arc::new(BufferCache::with_capacity(
        HostWait::new(device.clone()),
        SIZE,
    ));
    let requests = Arc::new(AtomicUsize::new(0));
    let group = WriteGroup::hold(&*device, {
        let (c, d, requests) = (Arc::downgrade(&cache), device.clone(), requests.clone());
        Arc::new(move || {
            // Reenter cache and device admission: the callback holds neither.
            c.upgrade().unwrap().flush(&*d).unwrap();
            requests.fetch_add(1, Ordering::SeqCst);
        })
    });
    cache.write(&*device, group.clone(), 1, &[1; SIZE]).unwrap();
    for _ in 0..2 {
        cache.read(&*device, 2, &mut [0; SIZE]).unwrap();
        assert_eq!(
            cache.write(&*device, group.clone(), 3, &[3; SIZE]),
            Err(BlockError::OutOfMemory)
        );
    }
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert_eq!(cache.resident_bytes(), SIZE);
    assert!(!device
        .events()
        .iter()
        .any(|e| matches!(e, Event::Issue(Op::Write(_)))));
    assert_eq!(cache.status(&*device, 1), Some(BufferStatus::Dirty));
    cache.submit(&*device, &group).wait().unwrap();
    cache.write(&*device, group.clone(), 3, &[3; SIZE]).unwrap();
    cache.read(&*device, 4, &mut [0; SIZE]).unwrap();
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    assert_eq!(cache.status(&*device, 3), Some(BufferStatus::Dirty));
}
