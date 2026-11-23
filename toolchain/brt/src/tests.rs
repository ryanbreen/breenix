use super::{
    __atomic_compare_exchange, __atomic_exchange, __atomic_load, __atomic_store, buckets_for_span,
    BUCKET_BYTES, LOCK_COUNT,
};
use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::{mpsc, Arc, Barrier};
use std::thread;
use std::time::Duration;

#[test]
fn lock_bucket_covers_object_span() {
    // The object starts in the last bucket and wraps into the first bucket.
    // The lower-addressed lock in the table must be acquired first.
    let boundary = BUCKET_BYTES * LOCK_COUNT;
    assert_eq!(
        buckets_for_span(boundary - 1, 2).collect::<Vec<_>>(),
        [0, LOCK_COUNT - 1],
    );
    assert_eq!(
        buckets_for_span(boundary - 1, BUCKET_BYTES + 2).collect::<Vec<_>>(),
        [0, 1, LOCK_COUNT - 1],
    );
    assert_eq!(
        buckets_for_span(boundary - 1, boundary + 1).collect::<Vec<_>>(),
        (0..LOCK_COUNT).collect::<Vec<_>>(),
    );
}

const SIZE: usize = 24;
const ACQUIRE: i32 = 2;
const RELEASE: i32 = 3;
const ACQ_REL: i32 = 4;
const SEQ_CST: i32 = 5;

#[repr(align(64))]
struct Storage(UnsafeCell<[u8; 2 * BUCKET_BYTES]>);

// SAFETY: The test accesses the shared bytes through brt entry points,
// under the locks for the same object address and size.
unsafe impl Sync for Storage {}

impl Storage {
    fn object(&self) -> *mut c_void {
        self.0
            .get()
            .cast::<u8>()
            .wrapping_add(BUCKET_BYTES - 8)
            .cast()
    }
}

#[test]
fn spanning_operations_under_contention_keep_every_update() {
    let storage = Arc::new(Storage(UnsafeCell::new([0; 2 * BUCKET_BYTES])));
    let mut initial = [0u8; SIZE];
    initial[SIZE - 1] = !initial[0];
    // SAFETY: The object spans two buckets and initial is a disjoint SIZE-byte input.
    unsafe { __atomic_store(SIZE, storage.object(), initial.as_mut_ptr().cast(), RELEASE) };

    let barrier = Arc::new(Barrier::new(2));
    let (finished, completions) = mpsc::channel();
    for _ in 0..2 {
        let (storage, barrier, finished) = (storage.clone(), barrier.clone(), finished.clone());
        thread::spawn(move || {
            let object = storage.object();
            let mut expected = [0u8; SIZE];
            // SAFETY: expected is a disjoint SIZE-byte output for the shared object.
            unsafe { __atomic_load(SIZE, object, expected.as_mut_ptr().cast(), ACQUIRE) };
            // Both threads start CAS with the same value, forcing one to retry.
            barrier.wait();
            for _ in 0..100 {
                loop {
                    assert_eq!(expected[SIZE - 1], !expected[0]);
                    let mut desired = expected;
                    desired[0] += 1;
                    desired[SIZE - 1] = !desired[0];
                    // SAFETY: expected and desired are disjoint, initialized SIZE-byte
                    // buffers. Only brt accesses the shared object.
                    let changed = unsafe {
                        __atomic_compare_exchange(
                            SIZE,
                            object,
                            expected.as_mut_ptr().cast(),
                            desired.as_mut_ptr().cast(),
                            SEQ_CST,
                            ACQUIRE,
                        )
                    };
                    if changed {
                        expected = desired;
                        break;
                    }
                }
            }
            finished.send(()).unwrap();
        });
    }
    drop(finished);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    for _ in 0..2 {
        completions
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("both spanning operations must finish");
    }

    let mut replacement = [42u8; SIZE];
    let mut result = [0u8; SIZE];
    // SAFETY: replacement and result are disjoint SIZE-byte buffers and the
    // object's storage remains live until after both buffers are checked.
    unsafe {
        __atomic_exchange(
            SIZE,
            storage.object(),
            replacement.as_mut_ptr().cast(),
            result.as_mut_ptr().cast(),
            ACQ_REL,
        )
    };
    initial[0] = 200;
    initial[SIZE - 1] = !initial[0];
    assert_eq!(result, initial);
    // SAFETY: result is a disjoint SIZE-byte output for the same live object.
    unsafe { __atomic_load(SIZE, storage.object(), result.as_mut_ptr().cast(), ACQUIRE) };
    assert_eq!(result, replacement);
}
