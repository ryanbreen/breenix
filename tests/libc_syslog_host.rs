//! Drive the hosted libc's logging calls at the file and formatting boundaries.
#![feature(c_variadic)]

#[path = "../libs/libbreenix-libc/src/hosted_syslog.rs"]
mod hosted_syslog;

use core::ffi::{CStr, VaList};
use std::cell::{Cell, RefCell};
use std::sync::Mutex;

static SERIAL: Mutex<()> = Mutex::new(());

#[derive(Debug, PartialEq)]
enum Event {
    Open(&'static str),
    Write(i32, Vec<u8>),
    Close(i32),
}

thread_local! {
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    static ERRNO: Cell<i32> = const { Cell::new(0) };
    static REJECT_LOG: Cell<bool> = const { Cell::new(false) };
}

fn events() -> Vec<Event> {
    EVENTS.take()
}

fn getpid() -> i32 {
    77
}

unsafe fn open(path: *const u8, flags: i32, mode: u32) -> i32 {
    assert_eq!(flags, 0o2004401); // O_CLOEXEC | O_NONBLOCK | O_NOCTTY | O_WRONLY
    assert_eq!(mode, 0);
    let path = unsafe { CStr::from_ptr(path.cast()) };
    match path.to_bytes() {
        b"/dev/kmsg" => {
            EVENTS.with_borrow_mut(|events| events.push(Event::Open("/dev/kmsg")));
            if REJECT_LOG.get() {
                ERRNO.set(13);
                -1
            } else {
                11
            }
        }
        b"/dev/console" => {
            EVENTS.with_borrow_mut(|events| events.push(Event::Open("/dev/console")));
            12
        }
        _ => panic!("unexpected log path {path:?}"),
    }
}

unsafe fn write(fd: i32, data: *const u8, len: usize) -> isize {
    let record = unsafe { core::slice::from_raw_parts(data, len) };
    EVENTS.with_borrow_mut(|events| events.push(Event::Write(fd, record.to_vec())));
    len as isize
}

fn close(fd: i32) -> i32 {
    EVENTS.with_borrow_mut(|events| events.push(Event::Close(fd)));
    0
}

mod pthread {
    pub fn errno() -> i32 {
        super::ERRNO.get()
    }
    pub fn set_errno(error: i32) {
        super::ERRNO.set(error);
    }
    pub struct RecursiveLock;
    impl RecursiveLock {
        pub const fn new() -> Self {
            Self
        }
        pub fn lock(&self) {}
        pub fn unlock(&self) {}
    }
}

mod string {
    pub unsafe fn strerror_r(errno: i32, out: *mut u8, room: usize) -> i32 {
        assert_eq!(errno, 2);
        let bytes = b"missing 100%\0";
        assert!(room >= bytes.len());
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
        0
    }
}

mod stdio {
    pub mod format {
        pub unsafe fn vsnprintf(out: *mut u8, room: usize, fmt: *const u8,
                                mut args: super::super::VaList<'_, '_>) -> i32 {
            let format = unsafe { super::super::CStr::from_ptr(fmt.cast()) };
            let body: &[u8] = match format.to_bytes() {
                b"value=%d, missing 100%%, %%m" => {
                    assert_eq!(unsafe { args.arg::<i32>() }, 17);
                    b"value=17, missing 100%, %m"
                }
                b"fallback" => b"fallback",
                _ => panic!("unexpected expanded log format {format:?}"),
            };
            assert!(room > body.len());
            unsafe {
                core::ptr::copy_nonoverlapping(body.as_ptr(), out, body.len());
                out.add(body.len()).write(0);
            }
            body.len() as i32
        }
    }
}

#[test]
fn openlog_opens_immediately_only_for_ndelay() {
    let _serial = SERIAL.lock().unwrap();
    hosted_syslog::closelog();
    unsafe { hosted_syslog::openlog(c"deferred".as_ptr(), 4, 8) };
    assert!(events().is_empty());
    unsafe { hosted_syslog::openlog(c"immediate".as_ptr(), 8, 8) };
    assert_eq!(events(), [Event::Open("/dev/kmsg")]);
    hosted_syslog::closelog();
}

#[test]
fn closelog_releases_the_logging_descriptor_once() {
    let _serial = SERIAL.lock().unwrap();
    unsafe { hosted_syslog::openlog(c"test".as_ptr(), 8, 8) };
    assert_eq!(events(), [Event::Open("/dev/kmsg")]);
    hosted_syslog::closelog();
    hosted_syslog::closelog();
    assert_eq!(events(), [Event::Close(11)]);
}

#[test]
fn syslog_writes_a_tagged_message_and_uses_the_requested_console_fallback() {
    let _serial = SERIAL.lock().unwrap();
    ERRNO.set(2);
    unsafe {
        hosted_syslog::openlog(c"cargo".as_ptr(), 1 | 2, 16 << 3);
        hosted_syslog::syslog(3, c"value=%d, %m, %%m".as_ptr(), 17i32);
    }
    assert_eq!(events(), [
        Event::Open("/dev/kmsg"),
        Event::Write(11, b"<131>cargo[77]: value=17, missing 100%, %m\n".to_vec()),
    ]);
    hosted_syslog::closelog();
    assert_eq!(events(), [Event::Close(11)]);

    REJECT_LOG.set(true);
    unsafe {
        hosted_syslog::openlog(c"cargo".as_ptr(), 2, 8);
        hosted_syslog::syslog((16 << 3) | 4, c"fallback".as_ptr());
    }
    assert_eq!(events(), [
        Event::Open("/dev/kmsg"),
        Event::Open("/dev/console"),
        Event::Write(12, b"<132>cargo: fallback\n".to_vec()),
        Event::Close(12),
    ]);
    assert_eq!(ERRNO.get(), 2);
    hosted_syslog::closelog();
}
