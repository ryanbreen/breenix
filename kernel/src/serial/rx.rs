use conquer_once::spin::OnceCell;
use crossbeam_queue::ArrayQueue;
use futures_util::task::AtomicWaker;

// Serial input queue and waker (similar to keyboard)
pub(super) static SERIAL_INPUT_QUEUE: OnceCell<ArrayQueue<u8>> = OnceCell::uninit();
static SERIAL_WAKER: AtomicWaker = AtomicWaker::new();

/// Called by the serial interrupt handler
///
/// Must not block or allocate.
pub fn add_serial_byte(byte: u8) {
    if let Ok(queue) = SERIAL_INPUT_QUEUE.try_get() {
        if let Err(_) = queue.push(byte) {
            log::warn!(target: "kernel::serial", "Serial input queue full; dropping input");
        } else {
            SERIAL_WAKER.wake();
        }
    } else {
        log::warn!(target: "kernel::serial", "Serial input queue uninitialized");
    }
}

// Serial input stream implementation
use core::{
    pin::Pin,
    task::{Context, Poll},
};
use futures_util::stream::Stream;

#[allow(dead_code)] // Used in serial_command_task (conditionally compiled)
pub struct SerialInputStream {
    _private: (),
}

impl SerialInputStream {
    #[allow(dead_code)] // Used in serial_command_task (conditionally compiled)
    pub fn new() -> Self {
        // Ensure queue is initialized
        let _ = SERIAL_INPUT_QUEUE.try_init_once(|| ArrayQueue::new(256));

        SerialInputStream { _private: () }
    }
}

impl Stream for SerialInputStream {
    type Item = u8;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<u8>> {
        let queue = SERIAL_INPUT_QUEUE
            .try_get()
            .expect("serial input queue not initialized");

        // Fast path
        if let Some(byte) = queue.pop() {
            return Poll::Ready(Some(byte));
        }

        SERIAL_WAKER.register(&cx.waker());
        match queue.pop() {
            Some(byte) => {
                SERIAL_WAKER.take();
                Poll::Ready(Some(byte))
            }
            None => Poll::Pending,
        }
    }
}
