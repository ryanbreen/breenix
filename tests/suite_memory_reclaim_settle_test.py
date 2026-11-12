"""A settled reading must include receipts temporarily owned by the drain."""
from pathlib import Path
import unittest

from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


def method(source, name):
    start = source.index(f'    pub(crate) fn {name}(')
    opening = source.index('{', start)
    depth = 1
    end = opening + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


class ReclaimSettle(unittest.TestCase):
    def test_sample_waits_for_exited_address_space_until_receipt_completes(self):
        # Run the production reader and settle loop with a virtual clock and
        # proc files. One exited address space owns 44 leaves and 12 tables.
        # Its receipt leaves the pending queue at 300 ms, then finishes at
        # 600 ms. No wall-clock scheduling or guest is involved.
        source = (ROOT / 'libs/libbreenix/src/suite_memory.rs').read_text()
        source = source.replace('std::fs::read_to_string', 'crate::fixture::read_to_string')
        process = (ROOT / 'kernel/src/process/process.rs').read_text()
        manager = (ROOT / 'kernel/src/process/manager.rs').read_text()
        methods = '\n'.join(method(process, name) for name in (
            'note_receipt_created', 'outstanding_retirement_receipts',
            'settle_retirement_receipt'))
        census = method(manager, 'outstanding_retirement_receipts')
        proc = (ROOT / 'kernel/src/fs/procfs/mod.rs').read_text()
        self.assertIn('manager.outstanding_retirement_receipts()', proc)
        self.assertIn('reclaim outstanding={}', proc)
        run_rust(self, r'''
mod types { pub type Fd = u32; }
mod error { pub type Error = std::io::Error; }
mod fs {
    pub const O_RDONLY: u32 = 0;
    pub fn open(_: &str, _: u32) -> Result<crate::types::Fd, crate::error::Error> { Ok(1) }
}
mod io {
    pub fn read(_: crate::types::Fd, _: &mut [u8]) -> Result<usize, crate::error::Error> { Ok(0) }
    pub fn close(_: crate::types::Fd) -> Result<(), crate::error::Error> { Ok(()) }
}
mod time {
    pub struct Stamp(u64);
    impl Stamp { pub fn as_nanos(&self) -> u128 { self.0 as u128 * 1_000_000 } }
    pub fn now_monotonic() -> Result<Stamp, ()> { Ok(Stamp(crate::fixture::now())) }
    pub fn sleep_ms(ms: u64) -> Result<(), ()> { crate::fixture::advance(ms); Ok(()) }
}
mod fixture {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{LazyLock, Mutex};
    struct Process { retirement_receipts: u32 }
    impl Process {
        @METHODS@
        // These fixture rows are already exited and reaped. Keep them resident
        // until their production receipt count permits the retirement join.
        fn latch_retired_if_settled(&mut self) -> bool { self.retirement_receipts == 0 }
    }
    struct Manager { processes: std::collections::BTreeMap<u64, Process> }
    impl Manager { @CENSUS@ }
    static MANAGER: LazyLock<Mutex<Manager>> = LazyLock::new(|| {
        let mut row = Process { retirement_receipts: 0 };
        row.note_receipt_created();
        Mutex::new(Manager { processes: [(17, row)].into_iter().collect() })
    });
    static NOW: AtomicU64 = AtomicU64::new(0);
    pub fn now() -> u64 { NOW.load(Ordering::Relaxed) }
    pub fn advance(ms: u64) {
        let prior = NOW.fetch_add(ms, Ordering::Relaxed);
        if prior < 600 && prior + ms >= 600 {
            assert!(MANAGER.lock().unwrap().processes.get_mut(&17).unwrap().settle_retirement_receipt());
        }
    }
    pub fn read_to_string(path: &str) -> std::io::Result<String> {
        let retained = if now() < 600 { 56 } else { 0 };
        match path {
            "/proc/meminfo" => Ok(format!("MemFree: {} kB\nKernelHeapFree: 1024 kB\n", (1000 - retained) * 4)),
            "/proc/breenix/frames" => Ok(format!(
                "owner user_pages {}\nowner page_tables {}\nreceipts pending={} parked=0\nreclaim outstanding={}\nboot settled=1\n",
                if retained != 0 { 44 } else { 0 },
                if retained != 0 { 12 } else { 0 },
                u8::from(now() < 300), MANAGER.lock().unwrap().outstanding_retirement_receipts())),
            _ => panic!("unexpected path {path}"),
        }
    }
}
mod memory {
@SOURCE@
}
#[test]
fn exited_root_cannot_disappear_from_settling_while_a_drain_owns_it() {
    let sample = memory::settled().unwrap();
    let record = sample.record("fixture", "after");
    println!("sample at {} ms: {record}", fixture::now());
    assert!(fixture::now() >= 300, "settle ignored the queued receipt");
    assert_eq!(sample.free, 1000,
        "settle sampled 44 user + 12 table pages still owned by the exited process's in-flight receipt");
    assert!(fixture::now() >= 700, "completion must precede the stable interval");
    assert!(sample.ownership_detail().is_empty());
    assert!(memory::verdict("fixture", Some(&sample), Some(&sample)).1);
}
'''.replace('@SOURCE@', source).replace('@METHODS@', methods).replace('@CENSUS@', census))


if __name__ == '__main__':
    unittest.main()
