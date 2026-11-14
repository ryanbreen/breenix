use std::pin::Pin;
use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};
use std::task::{Context, Poll, Wake, Waker};

use futures_util::stream::Stream;

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
    assert_eq!(Pin::new(&mut stream).poll_next(&mut cx), Poll::Ready(Some(b'a')));
    assert_eq!(Pin::new(&mut stream).poll_next(&mut cx), Poll::Pending);
    rx::add_serial_byte(b'b');
    assert_eq!(wake_count.0.load(Ordering::SeqCst), 1);
    assert_eq!(Pin::new(&mut stream).poll_next(&mut cx), Poll::Ready(Some(b'b')));
    assert_eq!(Pin::new(&mut stream).poll_next(&mut cx), Poll::Pending);
}
