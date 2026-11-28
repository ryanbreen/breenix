//! The production queue against the host device.

use crate::blk_queue::{
    request_type, status, Admission, BlkQueue, ChainStorage, Drain, Limits, RangeTable, RequestError,
    SubmitError, split_layout, split_queue_size, SplitLayout, MAX_SEGMENTS,
};
use crate::device::{self, Device, Dma, Handle, Ring, Taken};

const SIZE: u16 = 16;
const DISK_SECTORS: usize = 1024;
const MODERN: Limits = Limits { seg_max: 0, size_max: 0, exact_used_len: true };

/// Slot 0 and slot 2 have frames far apart (slot 2's in descending order);
/// slot 1's two frames are physically adjacent.
fn storage() -> Vec<ChainStorage> {
    let frames: [&[u64]; 3] = [&[0x10_000, 0x30_000], &[0x50_000, 0x51_000], &[0x70_000, 0x20_000]];
    frames
        .iter()
        .enumerate()
        .map(|(index, list)| {
            let mut chain = ChainStorage::EMPTY;
            chain.header = 0x1000 + 16 * index as u64;
            chain.status = 0x2000 + 16 * index as u64;
            chain.frames[..list.len()].copy_from_slice(list);
            chain.frame_count = list.len();
            chain
        })
        .collect()
}

fn setup(limits: Limits) -> (Handle, Device, BlkQueue<Ring>, Dma) {
    let mem = device::shared(SIZE);
    let dev = Device::new(&mem, DISK_SECTORS);
    let queue = BlkQueue::new(Ring(mem.clone()), SIZE, limits, &storage()).expect("queue setup");
    (mem.clone(), dev, queue, Dma(mem))
}

fn pattern(round: usize, slot: usize, len: usize) -> Vec<u8> {
    (0..len).map(|i| (round * 7 + slot * 31 + i * 13 + i / 512) as u8).collect()
}

/// The device's return order for a window of three chains.
fn order(round: usize) -> [usize; 3] {
    let rotate = round % 3;
    let mut order = [rotate, (rotate + 1) % 3, (rotate + 2) % 3];
    if round % 2 == 1 {
        order.reverse();
    }
    order
}

fn by_head(taken: &[Taken], head: u16) -> &Taken {
    taken.iter().find(|chain| chain.head == head).expect("device took the chain")
}

#[test]
fn chains_over_fragmented_frames_complete_out_of_order_across_index_wrap() {
    let (mem, mut dev, mut queue, mut dma) = setup(MODERN);
    let mut model = vec![0u8; DISK_SECTORS * 512];
    // Six chains a round: the 16-bit indices wrap more than once.
    let rounds = 11_000;
    let mut chains_seen = 0usize;
    for round in 0..rounds {
        let full = round % 997 == 0;
        let len = if full { 8192 } else { 512 };
        let sectors: Vec<u64> = (0..3).map(|slot| ((round * 48 + slot * 16) % (DISK_SECTORS - 16)) as u64).collect();
        let failing = if round % 5 == 0 { Some(order(round)[1]) } else { None };

        // A window of three writes, made visible by one index store.
        let slots: Vec<usize> = (0..3).map(|_| queue.reserve(7).expect("a free slot")).collect();
        assert_eq!(slots, [0, 1, 2]);
        assert_eq!(queue.reserve(7), None, "every slot is in use");
        let mut heads = [0u16; 3];
        for slot in 0..3 {
            let data = pattern(round, slot, len);
            heads[slot] = queue.submit(&mut dma, slot, request_type::OUT, sectors[slot], Some(&data), len).unwrap();
        }
        let stores = mem.borrow().idx_stores.len();
        assert!(queue.publish());
        assert!(!queue.publish(), "nothing new to publish");
        {
            let shared = mem.borrow();
            assert_eq!(shared.idx_stores.len(), stores + 1, "one index store per window");
            assert!(shared.idx_stores.last().unwrap().1, "barrier before the index store");
        }
        let taken = dev.take();
        assert_eq!(taken.len(), 3);
        chains_seen += 3;
        for slot in 0..3 {
            let chain = by_head(&taken, heads[slot]);
            assert_eq!((chain.kind, chain.sector), (request_type::OUT, sectors[slot]));
            let bytes: u32 = chain.data.iter().map(|d| d.len).sum();
            assert_eq!(bytes as usize, len);
            if full {
                // Adjacent frames share one descriptor; the others take one each.
                assert_eq!(chain.data.len(), if slot == 1 { 1 } else { 2 }, "slot {slot} segments");
                assert_eq!(chain.data[0].addr, storage()[slot].frames[0]);
            }
        }
        assert_eq!(queue.free_descriptors(), SIZE - if full { 11 } else { 9 });

        // The device returns them out of order; one may fail.
        let mut drained = Vec::new();
        for (step, &slot) in order(round).iter().enumerate() {
            let result = if failing == Some(slot) { status::IOERR } else { status::OK };
            dev.complete(by_head(&taken, heads[slot]), result, None);
            if step == 0 {
                assert_eq!(queue.drain(&mut dma, |s, _| drained.push(s)), Drain::Completed(1));
            }
        }
        assert_eq!(queue.drain(&mut dma, |s, _| drained.push(s)), Drain::Completed(2));
        assert_eq!(drained, order(round), "completions follow the device's order");
        assert_eq!(queue.free_descriptors(), SIZE, "every descriptor credit is back");
        assert_eq!(queue.drain(&mut dma, |_, _| panic!("nothing new")), Drain::Completed(0));
        for slot in 0..3 {
            let result = queue.finish(&mut dma, slot, None);
            if failing == Some(slot) {
                assert_eq!(result, Err(RequestError::Media), "slot {slot}'s own status");
            } else {
                assert_eq!(result, Ok(()), "slot {slot}'s own status");
                let at = sectors[slot] as usize * 512;
                model[at..at + len].copy_from_slice(&pattern(round, slot, len));
            }
        }
        assert_eq!(dev.disk, model);

        // Read the ranges back, returned in another order. In rounds divisible by
        // 11, one read's used length is short, which fails that read alone.
        let short = if round % 11 == 0 { Some(order(round + 1)[2]) } else { None };
        let slots: Vec<usize> = (0..3).map(|_| queue.reserve(7).unwrap()).collect();
        assert_eq!(slots, [0, 1, 2]);
        for slot in 0..3 {
            heads[slot] = queue.submit(&mut dma, slot, request_type::IN, sectors[slot], None, len).unwrap();
        }
        assert!(queue.publish());
        let taken = dev.take();
        chains_seen += 3;
        for &slot in order(round + 1).iter() {
            let chain = by_head(&taken, heads[slot]);
            dev.complete(chain, status::OK, if short == Some(slot) { Some(len as u32) } else { None });
        }
        let mut drained = Vec::new();
        assert_eq!(queue.drain(&mut dma, |s, _| drained.push(s)), Drain::Completed(3));
        assert_eq!(drained, order(round + 1));
        for slot in 0..3 {
            let mut out = vec![0xa5u8; len];
            let result = queue.finish(&mut dma, slot, Some(&mut out));
            if short == Some(slot) {
                assert_eq!(result, Err(RequestError::UsedLength(len as u32)));
                assert!(out.iter().all(|b| *b == 0xa5), "a failed read supplies no bytes");
            } else {
                assert_eq!(result, Ok(()));
                let at = sectors[slot] as usize * 512;
                assert_eq!(out, model[at..at + len], "round {round} slot {slot} data");
            }
        }
        assert_eq!(queue.free_slots(), 3);
        assert_eq!(queue.outstanding(), 0);
    }
    assert!(chains_seen > 65_536, "the 16-bit ring indices wrapped");
    assert_eq!(queue.published_idx(), chains_seen as u16);
    assert_eq!(queue.last_used(), chains_seen as u16);
}

#[test]
fn a_legacy_device_used_length_is_ignored() {
    let legacy = Limits { exact_used_len: false, ..MODERN };
    let (_mem, mut dev, mut queue, mut dma) = setup(legacy);
    assert_eq!(queue.size(), SIZE);
    dev.disk[..512].fill(0x5c);
    let slot = queue.reserve(7).unwrap();
    let head = queue.submit(&mut dma, slot, request_type::IN, 0, None, 512).unwrap();
    queue.publish();
    let taken = dev.take();
    dev.complete(by_head(&taken, head), status::OK, Some(0));
    assert_eq!(queue.drain(&mut dma, |_, _| {}), Drain::Completed(1));
    let mut out = vec![0; 512];
    assert_eq!(queue.finish(&mut dma, slot, Some(&mut out)), Ok(()));
    assert!(out.iter().all(|b| *b == 0x5c));
}

#[test]
fn an_abandoned_chain_keeps_its_memory_and_a_stale_used_entry_completes_nothing() {
    let (mem, mut dev, mut queue, mut dma) = setup(MODERN);
    // Two chains on the device, a third reserved but not issued.
    let a = queue.reserve(7).unwrap();
    let b = queue.reserve(7).unwrap();
    let c = queue.reserve(7).unwrap();
    let data = pattern(1, a, 8192);
    let head_a = queue.submit(&mut dma, a, request_type::OUT, 8, Some(&data), 8192).unwrap();
    let head_b = queue.submit(&mut dma, b, request_type::IN, 64, None, 4096).unwrap();
    queue.publish();
    let taken = dev.take();
    assert_eq!((queue.chains(), queue.chain_bytes()), (3, 8192));
    assert_eq!((queue.slot_head(a), queue.is_issued(b), queue.is_issued(c)), (Some(head_a), true, false));
    assert_eq!((queue.slot_state(a), queue.slot_state(c)), ('I', 'R'));

    // The boot wait gives up: the queue stops while the device holds both.
    queue.stop();
    let writes_at_stop = mem.borrow().bus.driver_writes.len();
    let descriptors_at_stop = queue.free_descriptors();

    // The device, not yet reset, still completes them.
    dev.complete(by_head(&taken, head_b), status::OK, None);
    dev.complete(by_head(&taken, head_a), status::OK, None);
    assert_eq!(queue.drain(&mut dma, |slot, _| panic!("slot {slot} completed after the stop")), Drain::Stopped);
    assert_eq!(queue.device_used_idx(), 2, "the device returned both");
    assert_eq!(queue.last_used(), 0, "the stopped queue read none of them");
    let mut out = vec![0xa5; 4096];
    assert_eq!(queue.finish(&mut dma, b, Some(&mut out)), Err(RequestError::Lost));
    assert_eq!(queue.finish(&mut dma, a, None), Err(RequestError::Lost));
    assert!(out.iter().all(|byte| *byte == 0xa5), "a lost read supplies no bytes");

    // Its memory stays owned: no slot, descriptor or buffer is reused.
    assert_eq!(queue.free_descriptors(), descriptors_at_stop);
    assert_eq!(queue.free_slots(), 0);
    assert_eq!(queue.reserve(7), None);
    assert_eq!(queue.submit(&mut dma, c, request_type::IN, 0, None, 512), Err(SubmitError::Stopped));
    assert!(!queue.publish());
    assert_eq!(mem.borrow().bus.driver_writes.len(), writes_at_stop, "the driver wrote no chain memory");
    assert_eq!(queue.finish(&mut dma, a, None), Err(RequestError::Lost), "still lost");
    assert_eq!(queue.slot_state(a), 'L');
}

#[test]
fn an_unknown_head_or_an_index_past_the_chains_issued_stops_the_queue() {
    for case in 0..2 {
        let (_mem, mut dev, mut queue, mut dma) = setup(MODERN);
        let a = queue.reserve(7).unwrap();
        let b = queue.reserve(7).unwrap();
        let head_a = queue.submit(&mut dma, a, request_type::IN, 0, None, 512).unwrap();
        let head_b = queue.submit(&mut dma, b, request_type::IN, 8, None, 512).unwrap();
        queue.publish();
        let taken = dev.take();
        dev.complete(by_head(&taken, head_a), status::OK, None);
        if case == 0 {
            // A head no chain of the driver's starts at.
            let stray = (0..SIZE).find(|h| *h != head_a && *h != head_b).unwrap();
            dev.post_raw(u32::from(stray), 513);
        } else {
            // More entries than chains published.
            dev.complete(by_head(&taken, head_b), status::OK, None);
            dev.post_raw(u32::from(head_b), 513);
        }
        let mut drained = Vec::new();
        assert_eq!(queue.drain(&mut dma, |slot, _| drained.push(slot)), Drain::Broken, "case {case}");
        assert!(queue.is_stopped());
        assert_eq!(queue.finish(&mut dma, b, Some(&mut [0; 512])), Err(RequestError::Lost));
        if case == 0 {
            // The entry before the stray one completed normally.
            assert_eq!(drained, [a]);
            let mut out = [0; 512];
            assert_eq!(queue.finish(&mut dma, a, Some(&mut out)), Ok(()));
        } else {
            assert!(drained.is_empty());
            assert_eq!(queue.finish(&mut dma, a, Some(&mut [0; 512])), Err(RequestError::Lost));
        }
        assert_eq!(queue.reserve(7), None);
        assert_eq!(queue.drain(&mut dma, |_, _| panic!("stopped")), Drain::Stopped);
    }
}

#[test]
fn the_negotiated_limits_shape_each_chain() {
    // seg_max 1: one data descriptor per request, even over adjacent frames.
    let one = Limits { seg_max: 1, ..MODERN };
    assert_eq!(one.chain_shape(2), (1, 4096));
    // size_max 2048: each frame takes two descriptors.
    let half = Limits { size_max: 2048, ..MODERN };
    assert_eq!(half.chain_shape(2), (4, 8192));
    assert_eq!(MODERN.chain_shape(MAX_SEGMENTS), (MAX_SEGMENTS, MAX_SEGMENTS * 4096));
    assert_eq!(MODERN.chains_for(128, MAX_SEGMENTS), 7);
    assert_eq!(MODERN.chains_for(256, MAX_SEGMENTS), 14);
    // Queue sizes are powers of two; the legacy layout aligns the used ring.
    assert_eq!((split_queue_size(1024, 256), split_queue_size(200, 256), split_queue_size(0, 256)), (256, 128, 0));
    assert_eq!(split_layout(256, 4096), SplitLayout { avail: 4096, used: 8192, bytes: 8192 + 6 + 8 * 256 });
    assert_eq!(split_layout(128, 4096), SplitLayout { avail: 2048, used: 4096, bytes: 4096 + 6 + 8 * 128 });

    // Three such chains need 18 descriptors; a 16-entry ring takes two.
    let mem = device::shared(SIZE);
    assert!(BlkQueue::new(Ring(mem.clone()), SIZE, half, &storage()).is_err());
    let mut dev = Device::new(&mem, DISK_SECTORS);
    let mut queue = BlkQueue::new(Ring(mem.clone()), SIZE, half, &storage()[..2]).expect("two chains fit");
    let mut dma = Dma(mem.clone());
    assert_eq!(queue.reserve(7), Some(0));
    queue.unreserve(0);
    let slot = queue.reserve(7).unwrap();
    assert_eq!(slot, 0, "an unsubmitted reservation is returned");
    let data = pattern(3, slot, 8192);
    let head = queue.submit(&mut dma, slot, request_type::OUT, 0, Some(&data), 8192).unwrap();
    queue.publish();
    let taken = dev.take();
    let chain = by_head(&taken, head);
    assert_eq!(chain.data.iter().map(|d| (d.addr, d.len)).collect::<Vec<_>>(),
        [(0x10_000, 2048), (0x10_800, 2048), (0x30_000, 2048), (0x30_800, 2048)]);
    assert_eq!(queue.submit(&mut dma, 1, request_type::IN, 0, None, 512), Err(SubmitError::NotReserved));
    drop(mem);
}

#[test]
fn overlapping_requests_wait_and_a_flush_covers_every_write_in_flight() {
    let mut table: RangeTable<4> = RangeTable::new();
    let Admission::Admitted(write) = table.try_admit(100, 108, true) else { panic!("first write") };
    assert_eq!(table.try_admit(104, 112, false), Admission::Wait, "a read of written sectors");
    assert_eq!(table.try_admit(107, 108, true), Admission::Wait, "an overlapping write");
    let Admission::Admitted(disjoint) = table.try_admit(108, 116, true) else { panic!("disjoint write") };
    let Admission::Admitted(read) = table.try_admit(0, 8, false) else { panic!("disjoint read") };
    let Admission::Admitted(shared) = table.try_admit(4, 12, false) else { panic!("reads share") };
    assert_eq!(table.try_admit(0, 4, true), Admission::Wait, "a write under reads");
    // A flush is a write of the whole device: it waits for the four admitted...
    assert_eq!(table.try_admit(0, u64::MAX, true), Admission::Wait);
    for index in [write, disjoint, read] {
        table.release(index);
        assert_eq!(table.try_admit(0, u64::MAX, true), Admission::Wait);
    }
    table.release(shared);
    let Admission::Admitted(flush) = table.try_admit(0, u64::MAX, true) else { panic!("flush") };
    // ...and holds later requests until it completes.
    assert_eq!(table.try_admit(500, 501, false), Admission::Wait);
    table.release(flush);
    assert!(table.is_empty());
    // A full table makes the next request wait too.
    for start in 0..4 {
        assert!(matches!(table.try_admit(start * 10, start * 10 + 1, false), Admission::Admitted(_)));
    }
    assert_eq!(table.try_admit(900, 901, false), Admission::Wait);
}

#[test]
fn a_slot_reports_the_tag_of_the_owner_whose_chain_completed() {
    // Owner P's chain completes on slot 0 and P takes its result. Owner X
    // reserves slot 0 next. The drain of X's chain must report X's tag: the
    // tag goes with the slot's ownership, so P, after `finish`, holds no
    // handle on what X armed.
    let (_mem, mut dev, mut queue, mut dma) = setup(MODERN);
    let p = queue.reserve(0x1111).unwrap();
    assert_eq!((p, queue.tag(p)), (0, 0x1111));
    let head = queue.submit(&mut dma, p, request_type::IN, 0, None, 512).unwrap();
    queue.publish();
    let taken = dev.take();
    dev.complete(by_head(&taken, head), status::OK, None);
    let mut drained = Vec::new();
    assert_eq!(queue.drain(&mut dma, |slot, tag| drained.push((slot, tag))), Drain::Completed(1));
    assert_eq!(drained, [(0, 0x1111)]);
    assert_eq!(queue.finish(&mut dma, p, Some(&mut [0; 512])), Ok(()));
    assert_eq!(queue.tag(p), 0, "taking the result ends the owner's tag");

    let x = queue.reserve(0x2222).unwrap();
    assert_eq!((x, queue.tag(x)), (0, 0x2222), "the next owner reuses slot 0");
    let head = queue.submit(&mut dma, x, request_type::IN, 8, None, 512).unwrap();
    queue.publish();
    let taken = dev.take();
    dev.complete(by_head(&taken, head), status::OK, None);
    drained.clear();
    assert_eq!(queue.drain(&mut dma, |slot, tag| drained.push((slot, tag))), Drain::Completed(1));
    assert_eq!(drained, [(0, 0x2222)], "the drain wakes the owner that armed the slot");

    // An unsubmitted reservation gives its tag back with the slot.
    assert_eq!(queue.finish(&mut dma, x, Some(&mut [0; 512])), Ok(()));
    let r = queue.reserve(0x3333).unwrap();
    queue.unreserve(r);
    assert_eq!(queue.tag(r), 0);
    // A lost chain keeps its tag, so a stop can still wake its owner.
    let l = queue.reserve(0x4444).unwrap();
    queue.submit(&mut dma, l, request_type::IN, 0, None, 512).unwrap();
    queue.publish();
    queue.stop();
    assert_eq!(queue.tag(l), 0x4444);
}

#[test]
fn a_used_entry_naming_a_staged_chain_stops_the_queue_and_keeps_the_chain() {
    // Chain A is published; chain B is built and its available entry
    // written, but `avail.idx` does not cover it yet. A device that returns
    // B's head has named a chain it was never given (section 2.7.8): the
    // driver must not complete B, wake its owner or recycle its
    // descriptors while the staged entry still points at them.
    let (mem, mut dev, mut queue, mut dma) = setup(MODERN);
    let a = queue.reserve(0xa).unwrap();
    let b = queue.reserve(0xb).unwrap();
    let head_a = queue.submit(&mut dma, a, request_type::IN, 0, None, 512).unwrap();
    queue.publish();
    let taken = dev.take();
    let head_b = queue.submit(&mut dma, b, request_type::OUT, 8, Some(&[0x5a; 512]), 512).unwrap();
    let descriptors_staged = queue.free_descriptors();
    dev.post_raw(u32::from(head_b), 1);
    let mut drained = Vec::new();
    assert_eq!(queue.drain(&mut dma, |slot, tag| drained.push((slot, tag))), Drain::Broken);
    assert!(drained.is_empty(), "the staged chain's owner was woken as if it completed");
    assert_eq!(queue.free_descriptors(), descriptors_staged, "the staged chain's descriptors were recycled");
    assert_eq!(queue.finish(&mut dma, b, None), Err(RequestError::Lost));
    assert!(!queue.publish(), "the stopped queue published the staged chain");
    assert_eq!(mem.borrow().avail_idx, 1);
    // The published chain the device really holds is lost with it.
    dev.complete(by_head(&taken, head_a), status::OK, None);
    assert_eq!(queue.finish(&mut dma, a, Some(&mut [0; 512])), Err(RequestError::Lost));
}

#[test]
fn a_stopped_queue_publishes_no_staged_chain() {
    // Caller W stages a write; before W publishes, another caller's wait is
    // abandoned and the queue stops, so W's chain is lost and W gets an
    // error. The device must then never see that write.
    let (mem, mut dev, mut queue, mut dma) = setup(MODERN);
    let w = queue.reserve(0x77).unwrap();
    queue.submit(&mut dma, w, request_type::OUT, 16, Some(&[0xc3; 1024]), 1024).unwrap();
    queue.stop();
    assert_eq!(queue.finish(&mut dma, w, None), Err(RequestError::Lost));
    assert!(!queue.publish(), "publish reported a notification for a stopped queue");
    assert!(mem.borrow().idx_stores.is_empty(), "the stopped queue stored avail.idx");
    assert!(dev.take().is_empty(), "the device was given a write its caller saw fail");
}

#[test]
fn a_small_ring_gets_a_shorter_chain_instead_of_no_device() {
    // Budget: 512 KiB of frames, as both drivers give a device.
    const BUDGET_FRAMES: usize = 128;
    // A 16-frame chain needs 18 descriptors, so a 16-entry ring fits none
    // of them: attach used to refuse the device on exactly this count.
    assert_eq!(MODERN.chains_for(16, MAX_SEGMENTS), 0);
    // The plan shortens the chain until one fits, and adds no frame that
    // could carry no data.
    assert_eq!(MODERN.chain_plan(16, MAX_SEGMENTS, BUDGET_FRAMES), Some((1, 14)));
    assert_eq!(MODERN.chain_plan(4, MAX_SEGMENTS, BUDGET_FRAMES), Some((1, 2)));
    assert_eq!(Limits { size_max: 2048, ..MODERN }.chain_plan(16, MAX_SEGMENTS, BUDGET_FRAMES), Some((1, 7)));
    assert_eq!(Limits { seg_max: 1, ..MODERN }.chain_plan(16, MAX_SEGMENTS, BUDGET_FRAMES), Some((5, 1)));
    // A large ring keeps 8 chains of 64 KiB.
    assert_eq!(MODERN.chain_plan(256, MAX_SEGMENTS, BUDGET_FRAMES), Some((8, MAX_SEGMENTS)));
    // Header, one data descriptor and status: a 2-entry ring fits nothing.
    assert_eq!(MODERN.chain_plan(2, MAX_SEGMENTS, BUDGET_FRAMES), None);

    // The planned chain runs a full-length write and read on the 16-entry ring.
    let (chains, frames) = MODERN.chain_plan(SIZE, MAX_SEGMENTS, BUDGET_FRAMES).unwrap();
    let mut chain = ChainStorage::EMPTY;
    chain.header = 0x1000;
    chain.status = 0x2000;
    for (index, frame) in chain.frames[..frames].iter_mut().enumerate() {
        *frame = 0x100_000 + 0x2000 * index as u64;
    }
    chain.frame_count = frames;
    let mem = device::shared(SIZE);
    let mut dev = Device::new(&mem, DISK_SECTORS);
    let mut queue = BlkQueue::new(Ring(mem.clone()), SIZE, MODERN, &vec![chain; chains]).expect("the planned chain fits");
    let mut dma = Dma(mem.clone());
    let len = queue.chain_bytes();
    assert_eq!(len, 14 * 4096);
    let data = pattern(9, 0, len);
    let slot = queue.reserve(1).unwrap();
    let head = queue.submit(&mut dma, slot, request_type::OUT, 0, Some(&data), len).unwrap();
    queue.publish();
    let taken = dev.take();
    dev.complete(by_head(&taken, head), status::OK, None);
    assert_eq!(queue.drain(&mut dma, |_, _| {}), Drain::Completed(1));
    assert_eq!(queue.finish(&mut dma, slot, None), Ok(()));
    assert_eq!(queue.free_descriptors(), SIZE);
    assert_eq!(&dev.disk[..len], &data[..]);
}
