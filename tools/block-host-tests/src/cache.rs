use crate::block::BlockDevice;
use crate::buffer_cache::{BufferCache, WriteGroup};
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
    let device = Arc::new(RecordingDevice::default());
    let cache = Arc::new(BufferCache::new(HostWait::new(device.clone())));
    device.hold(true, true);
    let earlier = {
        let (c, d) = (cache.clone(), device.clone());
        spawn("earlier", move || {
            c.write(&*d, WriteGroup::WriteThrough, 1, &[1; SIZE])
                .unwrap()
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
