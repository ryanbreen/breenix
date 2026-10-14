use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
std::thread_local! { static LEFT: Cell<Option<usize>> = const { Cell::new(None) }; }
struct TestAllocator;
fn allowed() -> bool {
    LEFT.try_with(|left| match left.get() {
        None => true,
        Some(0) => false,
        Some(n) => {
            left.set(Some(n - 1));
            true
        }
    })
    .unwrap_or(true)
}
unsafe impl GlobalAlloc for TestAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if allowed() {
            System.alloc(layout)
        } else {
            core::ptr::null_mut()
        }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if allowed() {
            System.realloc(ptr, layout, size)
        } else {
            core::ptr::null_mut()
        }
    }
}
#[global_allocator]
static ALLOCATOR: TestAllocator = TestAllocator;
fn with_budget<T>(n: usize, call: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            LEFT.set(None)
        }
    }
    LEFT.set(Some(n));
    let reset = Reset;
    let result = call();
    drop(reset);
    result
}
#[test]
fn fifo_open_oom_returns_enomem_without_endpoint_or_buffer_leaks() {
    for budget in 0..3 {
        let entry = alloc::sync::Arc::new(spin::Mutex::new(crate::fifo::FifoEntry::new()));
        let result = with_budget(budget, || crate::fifo::open_fifo_readwrite(&entry));
        assert!(matches!(result, crate::fifo::FifoOpenResult::Error(12)));
        let entry = entry.lock();
        assert_eq!((entry.readers, entry.writers), (0, 0));
        assert!(entry.buffer.is_none());
    }
}
#[test]
fn fifo_runtime_table_oom_returns_enomem_and_releases_inode_pin() {
    for budget in 0..2 {
        let registry = crate::fifo::FifoRegistry::new();
        let handle = crate::handle(11, 1);
        let pin = alloc::sync::Arc::downgrade(&handle.object);
        let result = with_budget(budget, || registry.get_or_create(handle));
        assert!(matches!(result, Err(12)));
        assert!(pin.upgrade().is_none());
    }
}
#[test]
fn read_waiter_oom_returns_enomem_without_publishing_a_waiter() {
    let mut pipe = crate::pipe::PipeBuffer::try_new_zero_refs().unwrap();
    assert_eq!(with_budget(0, || pipe.try_add_read_waiter(20)), Err(12));
    assert_eq!(pipe.try_add_read_waiter(20), Ok(()));
}

#[test]
fn ext2_fifo_oom_precedes_disk_changes() {
    // Every allocation a creation makes comes before its first disk write:
    // whichever one fails, the creation reports it and the disk is as it was.
    let mut budget = 0;
    loop {
        let (mut fs, bytes) = crate::fifo_disk::fixture();
        let before = bytes.lock().unwrap().clone();
        match with_budget(budget, || fs.create_node(2, "fifo", 0o010600, 0, 0, 0)) {
            Ok(_) => break,
            Err(error) => assert!(error.starts_with("No memory"), "budget {budget}: {error}"),
        }
        assert!(*bytes.lock().unwrap() == before, "budget {budget} changed the disk");
        budget += 1;
        assert!(budget < 64, "creation never succeeded");
    }
    assert!(budget > 2, "creation made only {budget} allocations");
}
#[test]
fn inode_index_oom_preserves_existing_identity() {
    let mut index = crate::inode_index::InodeIndex::new();
    assert_eq!(
        with_budget(0, || index.insert(11, 20)),
        Err("No memory for ext2 inode index")
    );
    assert!(index.get(&11).is_none());
    index.insert(11, 20).unwrap();
    assert_eq!(index.get(&11), Some(&20));
}
#[test]
fn checked_wait_oom_does_not_enqueue() {
    let mut waiters = alloc::collections::VecDeque::<u64>::new();
    assert!(!with_budget(0, || crate::waitqueue_alloc::reserve_one(
        &mut waiters
    )));
    assert!(waiters.is_empty());
}

#[test]
fn namespace_image_oom_is_reported_without_a_partial_copy() {
    assert_eq!(
        with_budget(0, || crate::namespace_alloc::copy_image(b"directory")),
        Err("No memory for namespace image")
    );
}
#[test]
fn namespace_record_mutation_does_not_allocate_names() {
    let (mut fs, _) = crate::fifo_disk::fixture();
    let ino = fs.create_node(2, "fifo", 0o010600, 0, 0, 0).unwrap();
    let mut data = fs.read_directory(&fs.read_inode(2).unwrap()).unwrap();
    assert_eq!(
        with_budget(0, || crate::ext2::dir::remove_entry(
            &mut data, "fifo", 4096
        )),
        Ok(ino)
    );
    assert!(with_budget(0, || crate::ext2::dir::is_directory_empty(
        &data
    )));
}
