//! Hosted lifecycle probe. Return from main so libc runs the exit callbacks.
#![deny(unsafe_op_in_unsafe_fn)]
#![feature(thread_local)]

use libbreenix::{errno::Errno, error::Error, io, process, types::Fd};
use std::cell::Cell;
use std::ffi::{c_char, c_void};
use std::process::ExitCode;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicI32, AtomicU64, AtomicU8, AtomicUsize, Ordering::Relaxed};

static PIPE_FD: AtomicI32 = AtomicI32::new(-1);
static EARLY_COUNT: AtomicUsize = AtomicUsize::new(0);
static EARLY_EVENTS: [AtomicU8; 4] = [const { AtomicU8::new(0) }; 4];
static WORKER_TID: AtomicU64 = AtomicU64::new(0);
#[thread_local]
static TLS_MARKER: Cell<u8> = Cell::new(1);

fn record(event: u8) {
    let fd = PIPE_FD.load(Relaxed);
    if fd < 0 {
        let slot = EARLY_COUNT.fetch_add(1, Relaxed);
        if let Some(entry) = EARLY_EVENTS.get(slot) {
            entry.store(event, Relaxed);
        }
        return;
    }

    // Each callback contributes one byte. A write failure leaves a missing byte
    // in the suite's receipt, even if it happened after main returned.
    while let Err(Error::Os(Errno::EINTR)) = io::write(Fd::from_raw(fd as u64), &[event]) {}
}

extern "C" fn preinit_first() {
    // Read the native PT_TLS template before main and leave a value main can check.
    if TLS_MARKER.get() == 1 {
        TLS_MARKER.set(2);
        record(b'a');
    }
}

extern "C" fn preinit_second() {
    record(b'b');
}

extern "C" fn init_first() {
    record(b'c');
}

extern "C" fn init_second() {
    record(b'd');
}

extern "C" fn fini_first() {
    record(b'y');
}

extern "C" fn fini_second() {
    record(b'z');
}

// gABI initialization/termination arrays hold pointers in call order,
// except fini, which is traversed backwards. The hosted script sorts the
// numeric init/fini input subsections before collecting plain entries.
#[used]
#[link_section = ".preinit_array"]
static PREINIT: [extern "C" fn(); 2] = [preinit_first, preinit_second];

#[used]
#[link_section = ".init_array.01000"]
static INIT_FIRST: extern "C" fn() = init_first;

#[used]
#[link_section = ".init_array.02000"]
static INIT_SECOND: extern "C" fn() = init_second;

#[used]
#[link_section = ".fini_array.01000"]
static FINI_FIRST: extern "C" fn() = fini_first;

#[used]
#[link_section = ".fini_array.02000"]
static FINI_SECOND: extern "C" fn() = fini_second;

extern "C" fn at_exit() {
    record(b'e');
}

unsafe extern "C" fn cxa_exit(_: *mut u8) {
    record(b'h');
}

unsafe extern "C" fn cxa_exit_later(_: *mut u8) {
    record(b'k');
}

unsafe extern "C" fn thread_exit(_: *mut u8) {
    record_worker_exit(b'x');
}

fn record_worker_exit(event: u8) {
    if process::gettid().is_ok_and(|tid| tid.raw() == WORKER_TID.load(Relaxed)) {
        record(event);
    }
}

struct ThreadDrop;

impl Drop for ThreadDrop {
    fn drop(&mut self) {
        record_worker_exit(b'r');
    }
}

thread_local! {
    static TLS: ThreadDrop = const { ThreadDrop };
}

// ELF gABI Elf64_Phdr and the LSB Core 5.0 dl_phdr_info prefix.
#[repr(C)]
struct Phdr {
    kind: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    paddr: u64,
    filesz: u64,
    memsz: u64,
    align: u64,
}

#[repr(C)]
struct PhdrInfo {
    addr: u64,
    name: *const c_char,
    phdr: *const Phdr,
    phnum: u16,
}

unsafe extern "C" fn visit(info: *mut PhdrInfo, _: usize, _: *mut c_void) -> i32 {
    // SAFETY: dl_iterate_phdr supplies the record and mapped header table
    // for the duration of this callback.
    let info = unsafe { &*info };
    // SAFETY: the table holds phnum native Elf64_Phdr entries.
    let headers = unsafe { std::slice::from_raw_parts(info.phdr, usize::from(info.phnum)) };
    // ELF gABI Table 7.2 and LSB Core 5.0 Table 11-1.
    const PT_LOAD: u32 = 1;
    const PT_TLS: u32 = 7;
    const PT_GNU_EH_FRAME: u32 = 0x6474_e550;
    i32::from(
        [PT_LOAD, PT_TLS, PT_GNU_EH_FRAME]
            .iter()
            .all(|&kind| headers.iter().any(|header| header.kind == kind)),
    )
}

extern "C" {
    fn __cxa_atexit(f: unsafe extern "C" fn(*mut u8), arg: *mut u8, dso: *mut u8) -> i32;
    fn __cxa_thread_atexit_impl(
        f: unsafe extern "C" fn(*mut u8),
        arg: *mut u8,
        dso: *mut u8,
    ) -> i32;
    fn atexit(f: extern "C" fn()) -> i32;
    fn dl_iterate_phdr(
        f: unsafe extern "C" fn(*mut PhdrInfo, usize, *mut c_void) -> i32,
        data: *mut c_void,
    ) -> i32;
}

fn main() -> ExitCode {
    let Some(fd) = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse::<i32>().ok())
    else {
        return ExitCode::from(2);
    };
    if fd < 0 || EARLY_COUNT.load(Relaxed) != EARLY_EVENTS.len() || TLS_MARKER.get() != 2 {
        return ExitCode::from(2);
    }
    PIPE_FD.store(fd, Relaxed);
    for event in &EARLY_EVENTS {
        record(event.load(Relaxed));
    }
    record(b'm');

    // SAFETY: visit uses only the iterator's record and ignores the context.
    if unsafe { dl_iterate_phdr(visit, null_mut()) } != 1 {
        return ExitCode::from(3);
    }
    // SAFETY: the callbacks are static and ignore their optional argument.
    if unsafe { __cxa_atexit(cxa_exit, null_mut(), null_mut()) } != 0
        || unsafe { atexit(at_exit) } != 0
        || unsafe { __cxa_atexit(cxa_exit_later, null_mut(), null_mut()) } != 0
    {
        return ExitCode::from(4);
    }
    record(b's');

    let worker = std::thread::Builder::new().spawn(|| {
        let Ok(tid) = process::gettid() else {
            return -1;
        };
        if TLS_MARKER.get() != 1 {
            return -1;
        }
        WORKER_TID.store(tid.raw(), Relaxed);
        TLS.with(|_| {});
        // SAFETY: the static callback ignores its argument and belongs to this thread.
        let result = unsafe { __cxa_thread_atexit_impl(thread_exit, null_mut(), null_mut()) };
        record(b't');
        result
    });
    let Ok(worker) = worker else {
        return ExitCode::from(5);
    };
    if !matches!(worker.join(), Ok(0)) {
        return ExitCode::from(5);
    }
    record(b'j');
    ExitCode::SUCCESS
}
