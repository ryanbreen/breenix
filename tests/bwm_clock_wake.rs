#[path = "../userspace/programs/src/bwm/clock_wake.rs"]
mod clock_wake;

use clock_wake::{ClockWake, Wait};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Fake { state: Mutex<(u32, bool)>, changed: Condvar }
impl Fake {
    fn step(&self) { let mut s = self.state.lock().unwrap(); s.0 += 1; self.changed.notify_all(); }
    fn stop(&self) { let mut s = self.state.lock().unwrap(); s.1 = true; self.changed.notify_all(); }
}
impl Wait for Arc<Fake> {
    fn value(&self) -> u32 { self.state.lock().unwrap().0 }
    fn wait(&self, expected: u32) -> bool {
        let mut s = self.state.lock().unwrap();
        while s.0 == expected && !s.1 { s = self.changed.wait(s).unwrap(); }
        !s.1
    }
}
#[repr(C)]
struct PollFd { fd: i32, events: i16, revents: i16 }
extern "C" { fn poll(fds: *mut PollFd, count: usize, timeout: i32) -> i32; }
fn readable(fd: i32, timeout: i32) -> bool {
    let mut p = PollFd { fd, events: 1, revents: 0 };
    unsafe { poll(&mut p, 1, timeout) > 0 && p.revents & 1 != 0 }
}
#[test]
fn step_wakes_and_no_step_stays_idle() {
    let fake = Arc::new(Fake::default());
    let wake = ClockWake::start(Some(fake.clone())).unwrap().unwrap();
    assert!(!readable(wake.fd(), 20));
    fake.step();
    assert!(readable(wake.fd(), 1000));
    wake.drain().unwrap();
    assert!(!readable(wake.fd(), 20));
    fake.step();
    assert!(readable(wake.fd(), 1000));
    wake.drain().unwrap();
    fake.stop();
    std::thread::sleep(Duration::from_millis(10));
}
#[test]
fn no_page_has_no_descriptor_or_worker() {
    assert!(ClockWake::start::<Arc<Fake>>(None).unwrap().is_none());
}

#[test]
fn worker_exit_reports_eof_so_the_caller_can_drop_the_descriptor() {
    let fake = Arc::new(Fake::default());
    let wake = ClockWake::start(Some(fake.clone())).unwrap().unwrap();
    fake.stop();
    let mut p = PollFd { fd: wake.fd(), events: 1, revents: 0 };
    assert_eq!(unsafe { poll(&mut p, 1, 1000) }, 1);
    assert_eq!(wake.drain().unwrap_err().kind(), std::io::ErrorKind::UnexpectedEof);
}
