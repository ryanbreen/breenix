use conquer_once::spin::OnceCell;
use core::{
    pin::Pin,
    task::{Context, Poll},
};
use crossbeam_queue::ArrayQueue;
use futures_util::{stream::Stream, task::AtomicWaker};

static SCANCODE_QUEUE: OnceCell<ArrayQueue<u8>> = OnceCell::uninit();
static WAKER: AtomicWaker = AtomicWaker::new();

/// Initialize the scancode queue early, before the keyboard task starts
pub(crate) fn init_queue() {
    SCANCODE_QUEUE
        .try_init_once(|| ArrayQueue::new(100))
        .expect("Scancode queue already initialized");
}

/// Called by the keyboard interrupt handler
///
/// Must not block or allocate.
///
/// NOTE: This function is currently unused because all keyboard processing
/// now happens directly in the interrupt handler to avoid modifier state
/// corruption. The async keyboard_task is kept for potential future use
/// (e.g., debug commands that can't run in interrupt context).
#[allow(dead_code)]
pub(crate) fn add_scancode(scancode: u8) {
    if let Ok(queue) = SCANCODE_QUEUE.try_get() {
        if let Err(_) = queue.push(scancode) {
            log::warn!("Scancode queue full; dropping keyboard input");
        } else {
            WAKER.wake();
        }
    } else {
        log::warn!("Scancode queue uninitialized");
    }
}

#[allow(dead_code)] // Used by keyboard_task (conditionally compiled)
pub struct ScancodeStream {
    _private: (),
}

impl ScancodeStream {
    #[allow(dead_code)] // Used by keyboard_task (conditionally compiled)
    pub fn new() -> Self {
        // Try to initialize, but it's ok if it's already initialized
        let _ = SCANCODE_QUEUE.try_init_once(|| ArrayQueue::new(100));

        ScancodeStream { _private: () }
    }
}

impl Stream for ScancodeStream {
    type Item = u8;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<u8>> {
        let queue = SCANCODE_QUEUE
            .try_get()
            .expect("scancode queue not initialized");

        // fast path
        if let Some(scancode) = queue.pop() {
            return Poll::Ready(Some(scancode));
        }

        WAKER.register(&cx.waker());
        match queue.pop() {
            Some(scancode) => {
                WAKER.take();
                Poll::Ready(Some(scancode))
            }
            None => Poll::Pending,
        }
    }
}

/// Wake the keyboard task - useful when returning control after userspace exit
pub fn wake_keyboard_task() {
    WAKER.wake();
}
