use breenfs_host_tests::{
    btree::{self, Error, NodeStore, ReadCause, ReadFailure},
    format::{Key, NODE_SIZE},
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    ptr,
};

thread_local! {
    static REFUSE_NODE_BUFFER: Cell<bool> = const { Cell::new(false) };
}

struct Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() == NODE_SIZE
            && layout.align() == 1
            && REFUSE_NODE_BUFFER.try_with(|armed| armed.replace(false)).unwrap_or(false)
        {
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
