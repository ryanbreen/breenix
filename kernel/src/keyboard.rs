use spin::Mutex;
use core::sync::atomic::{AtomicUsize, Ordering};

const QUEUE_SIZE: usize = 100;

static SCANCODE_QUEUE: Mutex<[u8; QUEUE_SIZE]> = Mutex::new([0; QUEUE_SIZE]);
static QUEUE_HEAD: AtomicUsize = AtomicUsize::new(0);
static QUEUE_TAIL: AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    // Reset queue pointers
    QUEUE_HEAD.store(0, Ordering::Release);
    QUEUE_TAIL.store(0, Ordering::Release);
}

/// Called by the keyboard interrupt handler
pub(crate) fn add_scancode(scancode: u8) {
    let mut queue = SCANCODE_QUEUE.lock();
    let head = QUEUE_HEAD.load(Ordering::Acquire);
    let tail = QUEUE_TAIL.load(Ordering::Acquire);
    
    let next_tail = (tail + 1) % QUEUE_SIZE;
    if next_tail != head {
        queue[tail] = scancode;
        QUEUE_TAIL.store(next_tail, Ordering::Release);
    } else {
        // Queue is full, drop the scancode
        log::warn!("Keyboard scancode queue full; dropping input");
    }
}

pub fn read_scancode() -> Option<u8> {
    let queue = SCANCODE_QUEUE.lock();
    let head = QUEUE_HEAD.load(Ordering::Acquire);
    let tail = QUEUE_TAIL.load(Ordering::Acquire);
    
    if head != tail {
        let scancode = queue[head];
        QUEUE_HEAD.store((head + 1) % QUEUE_SIZE, Ordering::Release);
        Some(scancode)
    } else {
        None
    }
}