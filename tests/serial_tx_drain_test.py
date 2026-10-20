#!/usr/bin/env python3
"""Exercise the production functions with host synchronization and device doubles."""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(source, name):
    start = re.search(r"fn " + name + r"(?:<[^>]*>)?\(", source).start()
    opened = source.index("{", start)
    depth = 1
    end = opened + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[start:end]


class Regression(unittest.TestCase):
    def test_race(self):
        source = (ROOT / "kernel/src/serial_line.rs").read_text()
        ring = source[source.index("struct TxRing {"):source.index("/// Held only around copies")]
        functions = "\n".join(function(source, name) for name in
                              ("with_ring", "send_queued", "send_queued_owned", "write_queued", "drain_output"))
        harness = r'''

use std::sync::{Mutex, MutexGuard, atomic::{AtomicBool, Ordering}, mpsc};
use std::time::Duration;
const CAPACITY: usize = 1024;
const TX_BYTES: usize = 8192;
const TX_HEADER: usize = 2;
struct Lock<T>(Mutex<T>);
impl<T> Lock<T> { const fn new(t:T)->Self { Self(Mutex::new(t)) } fn lock(&self)->MutexGuard<'_,T> { self.0.lock().unwrap() } }
static TX: Lock<TxRing> = Lock::new(TxRing::new());
static TX_STOPPED: AtomicBool = AtomicBool::new(false);
static UART: Mutex<()> = Mutex::new(());
static WIRE: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static ATTEMPT: Mutex<Option<mpsc::Sender<()>>> = Mutex::new(None);
struct Ownership { _guard: MutexGuard<'static, ()> }
impl Ownership { fn acquire()->Self {
    if let Some(tx) = ATTEMPT.lock().unwrap().take() { tx.send(()).unwrap(); }
    Self { _guard: UART.lock().unwrap() }
} }
struct Uart;
impl Uart { fn mask_interrupts()->u64 { 0 } fn restore_interrupts(_:u64) {} }
enum Writer { Tty }
enum Kind { Plain }
mod uart_owner { pub fn write<P, const T: bool>(_:super::Writer, bytes:&[u8], _:super::Kind, _: &mut super::Ownership) { let _:std::marker::PhantomData<P> = std::marker::PhantomData; super::WIRE.lock().unwrap().extend_from_slice(bytes); } }
mod task { pub mod scheduler { pub fn yield_current() { std::thread::yield_now(); } } }
fn main() {
    // The transmitter popped the last record and still owns the UART.
    assert!(TX.lock().push(b"in flight"));
    let mut owner = Ownership::acquire();
    let mut record = [0; CAPACITY];
    let n = TX.lock().pop(&mut record);
    let (attempt_tx, attempt_rx) = mpsc::channel();
    *ATTEMPT.lock().unwrap() = Some(attempt_tx);
    let (done_tx, done_rx) = mpsc::channel();
    let drain = std::thread::spawn(move || { drain_output(); done_tx.send(()).unwrap(); });
    attempt_rx.recv_timeout(Duration::from_secs(5)).expect("drain must acquire ownership even when the ring is empty");
    assert!(done_rx.try_recv().is_err(), "drain returned with a record in flight");
    uart_owner::write::<Uart, false>(Writer::Tty, &record[..n], Kind::Plain, &mut owner);
    TX.lock().in_flight -= 1;
    drop(owner);
    done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    drain.join().unwrap();
    assert_eq!(&*WIRE.lock().unwrap(), b"in flight");

    assert!(TX.lock().push(b" recovered"));
    let n = TX.lock().pop(&mut record);
    let (attempt_tx, attempt_rx) = mpsc::channel();
    *ATTEMPT.lock().unwrap() = Some(attempt_tx);
    let (done_tx, done_rx) = mpsc::channel();
    let drain = std::thread::spawn(move || { drain_output(); done_tx.send(()).unwrap(); });
    attempt_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut owner = Ownership::acquire();
    assert!(done_rx.try_recv().is_err(), "ownership recovery is not transmission completion");
    uart_owner::write::<Uart, false>(Writer::Tty, &record[..n], Kind::Plain, &mut owner);
    TX.lock().in_flight -= 1;
    drop(owner);
    done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    drain.join().unwrap();

    // Stopped output stays queued; drain waits until input resumes it.
    assert!(TX.lock().push(b" stopped"));
    TX_STOPPED.store(true, Ordering::Release);
    let (attempt_tx, attempt_rx) = mpsc::channel();
    *ATTEMPT.lock().unwrap() = Some(attempt_tx);
    let (done_tx, done_rx) = mpsc::channel();
    let drain = std::thread::spawn(move || { drain_output(); done_tx.send(()).unwrap(); });
    attempt_rx.recv_timeout(Duration::from_secs(5)).expect("stopped drain must wait");
    // Synchronize with a completed stopped pass through ownership.
    let owner = Ownership::acquire();
    assert!(done_rx.try_recv().is_err(), "drain returned while output was stopped");
    assert!(TX.lock().len != 0);
    TX_STOPPED.store(false, Ordering::Release);
    drop(owner);
    done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    drain.join().unwrap();
    assert_eq!(&*WIRE.lock().unwrap(), b"in flight recovered stopped");
}
'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / "race.rs").write_text(ring + "\n" + functions + "\n" + harness)
            subprocess.run(["rustc", "--edition=2021", "-A", "dead_code", str(path / "race.rs"), "-o", str(path / "race")], check=True)
            subprocess.run([str(path / "race")], check=True, timeout=20)


if __name__ == "__main__":
    unittest.main()
