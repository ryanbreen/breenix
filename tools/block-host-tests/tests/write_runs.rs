//! Publication through the cache issues one device call per LBA-contiguous run
//! (E24 ledger line 368, #3040): an 8 MiB fill used to cost 16,384 calls.
mod common;

use block_host_tests::block::BlockError;
use block_host_tests::cache::{BufferCache, BufferStatus, WriteGroup};
use common::{pattern, Call, CountingDevice, YieldWait, RUN_CAP_BLOCKS, RUN_CAP_BYTES, SIZE};
use std::sync::Arc;

fn writes(calls: &[Call]) -> Vec<(u64, usize)> {
    calls
        .iter()
        .filter_map(|call| match *call {
            Call::Write { start, count } => Some((start, count)),
            _ => None,
        })
        .collect()
}

#[test]
fn contiguous_8_mib_publication_takes_one_call_per_run() {
    const BYTES: usize = 8 * 1024 * 1024;
    const BLOCKS: u64 = (BYTES / SIZE) as u64;
    const FIRST: u64 = 40;
    let device = CountingDevice::new(FIRST + BLOCKS + 8);
    let cache = BufferCache::with_capacity(YieldWait::default(), 64 * 1024 * 1024);
    let group = WriteGroup::hold(&device, Arc::new(|| {}));
    for block in FIRST..FIRST + BLOCKS {
        cache.write(&device, group.clone(), block, &pattern(block)).unwrap();
    }
    assert!(writes(&device.take_calls()).is_empty(), "Hold staged bytes on the device");

    assert_eq!(cache.submit(&device, &group).wait(), Ok(()));

    let calls = writes(&device.take_calls());
    let cap = BYTES.div_ceil(RUN_CAP_BYTES);
    assert!(
        calls.len() <= cap,
        "8 MiB contiguous publication took {} device calls; the run cap allows {cap}",
        calls.len()
    );
    // The runs cover the publication exactly once, in LBA order, within the cap.
    let mut next = FIRST;
    for &(start, count) in &calls {
        assert_eq!(start, next, "runs out of order or overlapping: {calls:?}");
        assert!(count <= RUN_CAP_BLOCKS);
        next += count as u64;
    }
    assert_eq!(next, FIRST + BLOCKS);
    for block in FIRST..FIRST + BLOCKS {
        assert_eq!(device.media(block), pattern(block), "block {block} not on the device");
        assert_eq!(cache.status(&device, block), Some(BufferStatus::Clean));
    }
}

#[test]
fn failed_run_fails_every_entry_in_it() {
    let device = CountingDevice::new(64);
    let cache = BufferCache::with_capacity(YieldWait::default(), 4 * 1024 * 1024);
    let group = WriteGroup::hold(&device, Arc::new(|| {}));
    let run = 10..26u64;
    for block in run.clone().chain([40]) {
        cache.write(&device, group.clone(), block, &pattern(block)).unwrap();
    }
    device.fail_on(Some(15));

    assert_eq!(cache.submit(&device, &group).wait(), Err(BlockError::IoError));

    assert_eq!(writes(&device.take_calls()), [(10, 16), (40, 1)]);
    for block in run {
        assert_eq!(
            cache.status(&device, block),
            Some(BufferStatus::Failed),
            "block {block} of the failed run did not report the failure"
        );
        assert_eq!(cache.read(&device, block, &mut [0; SIZE]), Err(BlockError::IoError));
    }
    assert!(device.take_calls().is_empty(), "a failed entry was read from the device");
    // The separate run still completed, as a separate failed block did before.
    assert_eq!(cache.status(&device, 40), Some(BufferStatus::Clean));
    assert_eq!(device.media(40), pattern(40));
}
