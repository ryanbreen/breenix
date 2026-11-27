use breenfs_host_tests::{
    block::{BlockDevice, BlockError},
    btree::{self, Error, NodeStore, ReadCause, ReadFailure},
    commit::DeviceStore,
    format::{BlockPtr, Key, Pending, BLOCK_SIZE, NODE_SIZE, PENDING},
    scrub::Scrub,
    space,
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    collections::BTreeMap,
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

thread_local! {
    static REFUSE_NODE_BUFFER: Cell<bool> = const { Cell::new(false) };
    static REFUSE_RANGE_GROWTH: Cell<bool> = const { Cell::new(false) };
}

struct Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let refuse = match (layout.size(), layout.align()) {
            (NODE_SIZE, 1) => REFUSE_NODE_BUFFER.try_with(|armed| armed.replace(false)),
            (393_216, 8) => REFUSE_RANGE_GROWTH.try_with(|armed| armed.replace(false)),
            _ => Ok(false),
        };
        if refuse.unwrap_or(false) {
            return ptr::null_mut();
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

#[derive(Default)]
struct RangeStore {
    nodes: BTreeMap<u64, Vec<u8>>,
    last_addr: u64,
}

impl NodeStore for RangeStore {
    fn get(&self, addr: u64) -> Result<Vec<u8>, Error> {
        self.nodes.get(&addr).cloned().ok_or(Error::Store)
    }

    fn put(&mut self, addr: u64, bytes: Vec<u8>) -> Result<(), Error> {
        self.nodes.insert(addr, bytes);
        Ok(())
    }

    fn alloc(&mut self) -> Result<u64, Error> {
        self.last_addr = self.last_addr.max(20) + 4;
        Ok(self.last_addr)
    }
}

#[test]
fn pending_promotion_reports_failed_384_kib_range_growth() {
    let mut store = RangeStore::default();
    let mut root = btree::create(&mut store, 2, [1; 16], 2).unwrap();
    let pending = Pending { length: 1, released_generation: 1 }.encode();
    for offset in 1_000..1_000 + 4_097 {
        let key = Key { object: 0xffff_ffff_ffff_ff30, item_type: PENDING, offset };
        btree::insert(&mut store, &mut root, key, &pending).unwrap();
    }
    let before = root;
    REFUSE_RANGE_GROWTH.set(true);
    let result = space::promote_pending(&mut store, &mut root, 2);
    assert!(matches!(result, Err(space::Error::Tree(Error::Read(ReadFailure {
        cause: ReadCause::Allocation, addr,
    }))) if addr == before.addr));
    assert_eq!(root, before);
    assert_eq!(space::promote_pending(&mut store, &mut root, 2).unwrap().census(), Some((4_097, 1)));
}

#[derive(Default)]
struct ReadDevice(AtomicUsize);

impl BlockDevice for ReadDevice {
    fn read_block(&self, _: u64, _: &mut [u8]) -> Result<(), BlockError> {
        panic!("expected a single bulk read")
    }

    fn read_blocks(&self, start: u64, count: usize, buf: &mut [u8]) -> Result<(), BlockError> {
        assert_eq!((start, count, buf.len()), (24, NODE_SIZE / BLOCK_SIZE, NODE_SIZE));
        self.0.fetch_add(1, Ordering::Relaxed);
        buf.fill(42);
        Ok(())
    }

    fn write_block(&self, _: u64, _: &[u8]) -> Result<(), BlockError> {
        panic!("read-only test device")
    }

    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }

    fn num_blocks(&self) -> u64 {
        16_384
    }

    fn flush(&self) -> Result<(), BlockError> {
        panic!("read-only test device")
    }
}

#[test]
fn uncached_node_read_reports_allocation_failure_before_submitting_io() {
    let device = ReadDevice::default();
    let store = DeviceStore(&device);
    REFUSE_NODE_BUFFER.set(true);
    assert_eq!(
        store.get(24),
        Err(Error::Read(ReadFailure { cause: ReadCause::Allocation, addr: 24 }))
    );
    assert_eq!(device.0.load(Ordering::Relaxed), 0);
    assert_eq!(store.get(24).unwrap(), vec![42; NODE_SIZE]);
    assert_eq!(device.0.load(Ordering::Relaxed), 1);
}

#[test]
fn scrub_does_not_record_an_allocation_failure_as_corruption() {
    let device = ReadDevice::default();
    let root = BlockPtr { addr: 24, generation: 1, level: 0, csum: 0 };
    let mut scrub = Scrub::new();
    REFUSE_NODE_BUFFER.set(true);
    assert_eq!(
        scrub.step(&DeviceStore(&device), root, 1, NODE_SIZE),
        Err(Error::Read(ReadFailure { cause: ReadCause::Allocation, addr: 24 }))
    );
    assert_eq!(scrub.errors(), 0);
    assert!(scrub.completion().is_none());
}

#[derive(Default)]
struct Store {
    bytes: Option<Vec<u8>>,
    refuse_decode_buffer: bool,
}

impl NodeStore for Store {
    fn get(&self, _: u64) -> Result<Vec<u8>, Error> {
        let bytes = self.bytes.clone().ok_or(Error::Store)?;
        REFUSE_NODE_BUFFER.set(self.refuse_decode_buffer);
        Ok(bytes)
    }

    fn put(&mut self, _: u64, bytes: Vec<u8>) -> Result<(), Error> {
        self.bytes = Some(bytes);
        Ok(())
    }

    fn alloc(&mut self) -> Result<u64, Error> {
        Ok(24)
    }
}

#[test]
fn node_decode_reports_allocation_failure() {
    let mut store = Store::default();
    let root = btree::create(&mut store, 1, [1; 16], 1).unwrap();
    store.refuse_decode_buffer = true;
    let key = Key { object: 2, item_type: 1, offset: 0 };
    assert_eq!(
        btree::search(&store, root, key),
        Err(Error::Read(ReadFailure { cause: ReadCause::Allocation, addr: root.addr }))
    );
}

#[test]
fn node_encode_reports_allocation_failure_without_publishing_bytes() {
    let mut store = Store::default();
    REFUSE_NODE_BUFFER.set(true);
    assert_eq!(
        btree::create(&mut store, 1, [1; 16], 1),
        Err(Error::Read(ReadFailure { cause: ReadCause::Allocation, addr: 24 }))
    );
    assert!(store.bytes.is_none());
}
