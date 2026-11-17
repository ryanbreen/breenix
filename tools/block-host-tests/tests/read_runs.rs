//! A multi-block cache read serves hits from memory and reads each
//! LBA-contiguous run of misses with one device call (E24 ledger line 368, #3040).
mod common;

use block_host_tests::block::BlockError;
use block_host_tests::cache::BufferCache;
use common::{pattern, Call, CountingDevice, YieldWait, RUN_CAP_BLOCKS, SIZE};

fn reads(calls: &[Call]) -> Vec<(u64, usize)> {
    calls
        .iter()
        .filter_map(|call| match *call {
            Call::Read { start, count } => Some((start, count)),
            _ => None,
        })
        .collect()
}

fn expect(start: u64, count: usize) -> Vec<u8> {
    (start..start + count as u64).flat_map(pattern).collect()
}

#[test]
fn read_misses_take_one_call_per_contiguous_run() {
    let device = CountingDevice::patterned(1024);
    let cache = BufferCache::with_capacity(YieldWait::default(), 8 * 1024 * 1024);
    cache.read(&device, 4, &mut [0; SIZE]).unwrap();
    device.take_calls();
    let before = cache.counters();

    let mut bytes = vec![0; 12 * SIZE];
    cache.read_blocks(&device, 0, 12, &mut bytes).unwrap();

    assert_eq!(bytes, expect(0, 12));
    assert_eq!(reads(&device.take_calls()), [(0, 4), (5, 7)]);
    let after = cache.counters();
    assert_eq!(after.hits.count, before.hits.count + 1);
    assert_eq!(after.misses.count, before.misses.count + 11);
    assert_eq!(after.device_reads.count, before.device_reads.count + 2);

    // Blocks 0..12 are now resident: the same read issues no device call.
    cache.read_blocks(&device, 0, 12, &mut bytes).unwrap();
    assert!(device.take_calls().is_empty());
    assert_eq!(bytes, expect(0, 12));

    // A cold run longer than the cap splits at the cap.
    let count = 2 * RUN_CAP_BLOCKS + 1;
    let mut long = vec![0; count * SIZE];
    cache.read_blocks(&device, 100, count, &mut long).unwrap();
    assert_eq!(long, expect(100, count));
    let first = 100 + RUN_CAP_BLOCKS as u64;
    assert_eq!(
        reads(&device.take_calls()),
        [(100, RUN_CAP_BLOCKS), (first, RUN_CAP_BLOCKS), (first + RUN_CAP_BLOCKS as u64, 1)]
    );
}

#[test]
fn failed_read_run_publishes_none_of_its_entries() {
    let device = CountingDevice::patterned(1024);
    let cache = BufferCache::with_capacity(YieldWait::default(), 8 * 1024 * 1024);
    device.fail_on(Some(205));
    let mut bytes = vec![0; 10 * SIZE];

    assert_eq!(cache.read_blocks(&device, 200, 10, &mut bytes), Err(BlockError::IoError));
    assert_eq!(reads(&device.take_calls()), [(200, 10)]);
    for block in 200..210 {
        assert!(cache.status(&device, block).is_none(), "failed read published block {block}");
    }

    device.fail_on(None);
    cache.read_blocks(&device, 200, 10, &mut bytes).unwrap();
    assert_eq!(bytes, expect(200, 10));
    assert_eq!(reads(&device.take_calls()), [(200, 10)]);
}
