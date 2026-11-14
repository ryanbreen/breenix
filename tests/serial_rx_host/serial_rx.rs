use std::pin::Pin;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::task::{Context, Poll, Wake, Waker};

use futures_util::stream::Stream;

#[path = "../../kernel/src/input/host/progress.rs"]
#[allow(dead_code)] // Host substitutes only the scheduler observation/output boundary.
mod progress;

mod input {
    pub mod host {
        pub mod diagnostics {
            pub use crate::progress::Count;
            use crate::progress::{FirstFailure, Progress, COUNT};
            pub static PROGRESS: Progress = Progress::new();
            pub static FAILURE: FirstFailure<(usize, [u64; COUNT])> = FirstFailure::new();
            pub fn capture_overflow(len: usize) {
                FAILURE.capture(|| (len, PROGRESS.sample()));
            }
        }
    }
}

#[path = "../../kernel/src/serial/rx.rs"]
mod rx;

struct WakeCount(AtomicUsize);
impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn serial_input_delivers_queued_bytes_and_wakes_pending_consumer() {
    let mut stream = rx::SerialInputStream::new();
    let wake_count = Arc::new(WakeCount(AtomicUsize::new(0)));
    let waker = Waker::from(wake_count.clone());
    let mut cx = Context::from_waker(&waker);

    rx::add_serial_byte(b'a');
    assert_eq!(
        Pin::new(&mut stream).poll_next(&mut cx),
        Poll::Ready(Some(b'a'))
    );
    assert_eq!(Pin::new(&mut stream).poll_next(&mut cx), Poll::Pending);
    rx::add_serial_byte(b'b');
    assert_eq!(wake_count.0.load(Ordering::SeqCst), 1);
    assert_eq!(
        Pin::new(&mut stream).poll_next(&mut cx),
        Poll::Ready(Some(b'b'))
    );
    assert_eq!(Pin::new(&mut stream).poll_next(&mut cx), Poll::Pending);
    use input::host::diagnostics::{Count, FAILURE, PROGRESS};
    assert!(FAILURE.take().is_none());
    for _ in 0..256 {
        rx::add_serial_byte(b'c');
    }
    rx::add_serial_byte(b'd');
    rx::add_serial_byte(b'e');
    let snapshot = FAILURE.take();
    assert!(snapshot.is_some(), "RX overflow must freeze a snapshot");
    let (len, counts) = snapshot.unwrap();
    assert_eq!(len, 256);
    assert_eq!(counts[Count::Push as usize], 258);
    assert_eq!(counts[Count::Pop as usize], 2);
    assert_eq!(
        counts[Count::StreamPoll as usize],
        counts[Count::StreamReturn as usize]
    );
    assert_eq!(PROGRESS.sample()[Count::Push as usize], 258);
    assert!(FAILURE.take().is_none());
    for _ in 0..256 {
        assert_eq!(
            Pin::new(&mut stream).poll_next(&mut cx),
            Poll::Ready(Some(b'c'))
        );
    }
    assert_eq!(Pin::new(&mut stream).poll_next(&mut cx), Poll::Pending);
}
