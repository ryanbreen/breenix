//! Hosted system logging, from POSIX.1-2024 XSH syslog and XBD <syslog.h>.
//!
//! Breenix's local facility is the timestamped kernel log via /dev/kmsg.
//! Its existing privilege and single-record restrictions apply to callers.

use core::cell::UnsafeCell;
use core::ffi::{c_char, c_int, CStr, VaList};
use core::fmt::{self, Write};

use crate::pthread::{errno, set_errno, RecursiveLock};

const LOG_PID: c_int = 0x01;
const LOG_CONS: c_int = 0x02;
const LOG_NDELAY: c_int = 0x08;
const LOG_USER: c_int = 1 << 3;
const FACILITY_MASK: c_int = 0xf8;
const PRIORITY_MASK: c_int = 7;
const OPEN_FLAGS: c_int = 0o2004401; // O_CLOEXEC | O_NONBLOCK | O_NOCTTY | O_WRONLY
const MAX_RECORD: usize = 1024; // The Breenix kmsg record limit.

struct State {
    ident: [u8; 128],
    ident_len: usize,
    options: c_int,
    facility: c_int,
    fd: c_int,
}

impl State {
    const fn new() -> Self {
        Self {
            ident: [0; 128],
            ident_len: 0,
            options: 0,
            facility: LOG_USER,
            fd: -1,
        }
    }

    fn open(&mut self) {
        if self.fd < 0 {
            // SAFETY: the device pathname is NUL terminated and flags take no mode.
            self.fd = unsafe { crate::open(c"/dev/kmsg".as_ptr().cast(), OPEN_FLAGS, 0) };
        }
    }
}

struct Log {
    lock: RecursiveLock,
    state: UnsafeCell<State>,
}

// SAFETY: the lock serializes mutable access across threads. The operations
// inside use libc's internal non-cancelling file calls and formatter.
unsafe impl Sync for Log {}

static LOG: Log = Log {
    lock: RecursiveLock::new(),
    state: UnsafeCell::new(State::new()),
};

fn with_log(action: impl FnOnce(&mut State)) {
    let saved_errno = errno();
    LOG.lock.lock();
    // SAFETY: only the holder of LOG.lock accesses the state.
    action(unsafe { &mut *LOG.state.get() });
    LOG.lock.unlock();
    set_errno(saved_errno);
}

#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn openlog(ident: *const c_char, options: c_int, facility: c_int) {
    with_log(|log| {
        if !ident.is_null() {
            // SAFETY: the caller supplies a readable, NUL-terminated identifier.
            let ident = unsafe { CStr::from_ptr(ident) }.to_bytes();
            log.ident_len = ident.len().min(log.ident.len());
            log.ident[..log.ident_len].copy_from_slice(&ident[..log.ident_len]);
        }
        log.options = options;
        log.facility = facility & FACILITY_MASK;
        if options & LOG_NDELAY != 0 {
            log.open();
        }
    });
}

#[cfg_attr(not(test), no_mangle)]
pub extern "C" fn closelog() {
    with_log(|log| {
        let fd = core::mem::replace(&mut log.fd, -1);
        if fd >= 0 {
            crate::close(fd);
        }
    });
}

struct Record {
    bytes: [u8; MAX_RECORD],
    len: usize,
}

impl Record {
    fn append(&mut self, bytes: &[u8]) {
        let count = bytes.len().min(MAX_RECORD - 1 - self.len);
        self.bytes[self.len..self.len + count].copy_from_slice(&bytes[..count]);
        self.len += count;
    }

    fn finish(&mut self) -> &[u8] {
        // /dev/kmsg accepts UTF-8 and one record. Escape control characters
        // and invalid bytes in place so arbitrary C strings cannot split it.
        let record = &mut self.bytes[..self.len];
        for byte in record.iter_mut() {
            if byte.is_ascii_control() {
                *byte = b' ';
            }
        }
        let mut rest = record;
        while let Err(error) = core::str::from_utf8(rest) {
            let invalid = &mut rest[error.valid_up_to()..];
            let count = error.error_len().unwrap_or(invalid.len());
            invalid[..count].fill(b'?');
            rest = &mut invalid[count..];
        }
        self.bytes[self.len] = b'\n';
        &self.bytes[..=self.len]
    }
}

impl Write for Record {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.append(text.as_bytes());
        Ok(())
    }
}

// Replace only complete %m conversions, leaving %% and the argument order
// intact. Literal percent bytes in the errno message are themselves escaped.
fn expand_errno(format: &[u8], saved_errno: c_int, expanded: &mut [u8]) -> Option<usize> {
    let mut description = [0u8; 128];
    let mut cached_error = None;
    let mut written = 0;
    let mut remaining = format;
    while let Some((&first, tail)) = remaining.split_first() {
        if let (b'%', Some((&next @ (b'm' | b'%'), rest))) = (first, tail.split_first()) {
            remaining = rest;
            let literal: &[u8] = if next == b'm' {
                let len = *cached_error.get_or_insert_with(|| {
                    // SAFETY: description is writable for the given length.
                    unsafe {
                        crate::string::strerror_r(saved_errno, description.as_mut_ptr(), description.len());
                    }
                    description.iter().position(|&byte| byte == 0).unwrap_or(description.len())
                });
                &description[..len]
            } else {
                b"%"
            };
            for &byte in literal {
                *expanded.get_mut(written)? = byte;
                written += 1;
                if byte == b'%' {
                    *expanded.get_mut(written)? = byte;
                    written += 1;
                }
            }
        } else {
            *expanded.get_mut(written)? = first;
            written += 1;
            remaining = tail;
        }
    }
    *expanded.get_mut(written)? = 0;
    Some(written)
}

unsafe fn format_body(
    record: &mut Record,
    format: *const c_char,
    args: VaList<'_, '_>,
    saved_errno: c_int,
) -> bool {
    // SAFETY: syslog's caller provides a NUL-terminated printf format.
    let bytes = unsafe { CStr::from_ptr(format) }.to_bytes();
    let mut expanded = [0u8; MAX_RECORD * 4];
    let format = if bytes.windows(2).any(|pair| pair == b"%m") {
        if expand_errno(bytes, saved_errno, &mut expanded).is_none() {
            return false;
        }
        expanded.as_ptr()
    } else {
        format.cast()
    };
    let body = &mut record.bytes[record.len..];
    // SAFETY: the format has the original argument conversions and body is
    // writable for the given capacity, including its terminator.
    let written = unsafe {
        crate::stdio::format::vsnprintf(body.as_mut_ptr(), body.len(), format, args)
    };
    if written < 0 {
        return false;
    }
    record.len += (written as usize).min(body.len() - 1);
    true
}

#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C" fn syslog(priority: c_int, format: *const c_char, mut args: ...) {
    let saved_errno = errno();
    with_log(|log| {
        let priority = (priority & PRIORITY_MASK)
            | if priority & FACILITY_MASK == 0 {
                log.facility
            } else {
                priority & FACILITY_MASK
            };
        let mut record = Record { bytes: [0; MAX_RECORD], len: 0 };
        let _ = write!(record, "<{priority}>");
        if log.ident_len == 0 {
            record.append(b"syslog");
        } else {
            record.append(&log.ident[..log.ident_len]);
        }
        if log.options & LOG_PID != 0 {
            let _ = write!(record, "[{}]", crate::getpid());
        }
        record.append(b": ");
        // SAFETY: the caller supplies printf arguments matching the format.
        if !unsafe { format_body(&mut record, format, args.as_va_list(), saved_errno) } {
            return;
        }
        let message = record.finish();
        log.open();
        if log.fd >= 0 {
            // SAFETY: message is readable for its length.
            if unsafe { crate::write(log.fd, message.as_ptr(), message.len()) } == message.len() as isize {
                return;
            }
        }
        if log.options & LOG_CONS != 0 {
            // SAFETY: the device pathname is NUL terminated. O_NOCTTY
            // prevents acquiring the console as a controlling terminal.
            let fd = unsafe { crate::open(c"/dev/console".as_ptr().cast(), OPEN_FLAGS, 0) };
            if fd >= 0 {
                // SAFETY: message is readable for its length.
                unsafe { crate::write(fd, message.as_ptr(), message.len()) };
                crate::close(fd);
            }
        }
    });
}
