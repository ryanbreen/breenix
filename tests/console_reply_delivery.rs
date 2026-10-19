//! Run the kernel's delivery code with controlled input and grid contention.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Condvar, Mutex};
use std::time::Duration;

mod vt {
    pub const VT_COUNT: usize = 1;
}
struct Replies(VecDeque<u8>);
impl Replies {
    fn consume(&mut self, n: usize) {
        self.0.drain(..n);
    }
    fn peek(&self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.0.len());
        for (to, from) in out.iter_mut().zip(&self.0) {
            *to = *from;
        }
        n
    }
}
struct Vt {
    replies: Replies,
}
struct Console {
    vts: [Vt; 1],
}
static GRID: Mutex<Console> = Mutex::new(Console {
    vts: [Vt {
        replies: Replies(VecDeque::new()),
    }],
});
static INPUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static SERIAL: Mutex<()> = Mutex::new(());
static PAUSE: Mutex<Option<mpsc::Sender<()>>> = Mutex::new(None);
static RESUME: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
static FAIL_ACK: AtomicBool = AtomicBool::new(false);
static FAIL_INPUT: AtomicBool = AtomicBool::new(false);
thread_local! { static MASKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    MASKED.with(|masked| {
        let previous = masked.replace(true);
        let result = f();
        masked.set(previous);
        result
    })
}
fn with_grid<R>(f: impl FnOnce(&mut Console) -> R) -> Option<R> {
    if FAIL_ACK.swap(false, Ordering::SeqCst) {
        return None;
    }
    Some(f(&mut GRID.lock().unwrap()))
}
mod task {
    pub mod scheduler {
        pub fn yield_current() {
            assert!(!crate::MASKED.with(|masked| masked.get()));
            std::thread::yield_now();
        }
    }
}
mod tty {
    pub mod driver {
        pub fn push_reply(_: usize, byte: u8) -> bool {
            use super::super::*;
            assert!(
                MASKED.with(|masked| masked.get()),
                "delivery holder must not be preemptible"
            );
            let pause = PAUSE.lock().unwrap().take();
            if let Some(entered) = pause {
                entered.send(()).unwrap();
                let ready = RESUME.0.lock().unwrap();
                drop(RESUME.1.wait_while(ready, |ready| !*ready).unwrap());
            }
            if FAIL_INPUT.swap(false, Ordering::SeqCst) {
                return false;
            }
            INPUT.lock().unwrap().push(byte);
            // Fail removal of the first accepted byte, exercising its retry
            // without allowing the byte to be admitted a second time.
            if INPUT.lock().unwrap().len() == 1 {
                FAIL_ACK.store(true, Ordering::SeqCst);
            }
            true
        }
    }
}
#[path = "../kernel/src/console/replies.rs"]
mod replies;
fn queue(bytes: &[u8]) {
    GRID.lock().unwrap().vts[0].replies.0.extend(bytes);
    replies::REPLY_QUEUED.store(true, Ordering::SeqCst);
}
fn reset() {
    INPUT.lock().unwrap().clear();
    assert!(GRID.lock().unwrap().vts[0].replies.0.is_empty());
    FAIL_INPUT.store(true, Ordering::SeqCst);
}
#[test]
fn drain_eleven_reports_before_enter() {
    let _serial = SERIAL.lock().unwrap();
    reset();
    let reports = b"\x1b[1;3R".repeat(11);
    queue(&reports);
    replies::deliver_queued_replies();
    INPUT.lock().unwrap().push(b'\n');
    assert_eq!(*INPUT.lock().unwrap(), [reports, vec![b'\n']].concat());
    assert!(GRID.lock().unwrap().vts[0].replies.0.is_empty());
}
#[test]
fn writer_waits_for_active_render_delivery() {
    let _serial = SERIAL.lock().unwrap();
    reset();
    *RESUME.0.lock().unwrap() = false;
    let (entered, held) = mpsc::channel();
    *PAUSE.lock().unwrap() = Some(entered);
    queue(b"\x1b[1;3R");
    let holder = std::thread::spawn(replies::deliver_replies);
    held.recv_timeout(Duration::from_secs(5)).unwrap();
    // The holder cleared the hint but has not delivered the old report yet.
    assert!(!replies::REPLY_QUEUED.load(Ordering::SeqCst));
    queue(b"\x1b[1;9R");
    let (started, waiting) = mpsc::channel();
    let (returned, done) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        started.send(()).unwrap();
        replies::deliver_queued_replies();
        INPUT.lock().unwrap().push(b'\n');
        returned.send(()).unwrap();
    });
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    let premature = done.recv_timeout(Duration::from_millis(50)).is_ok();
    *RESUME.0.lock().unwrap() = true;
    RESUME.1.notify_all();
    holder.join().unwrap();
    writer.join().unwrap();
    assert!(
        !premature,
        "write returned while the render holder was suspended"
    );
    assert_eq!(*INPUT.lock().unwrap(), b"\x1b[1;3R\x1b[1;9R\n");
}
