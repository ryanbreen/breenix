//! Stream state and shared locked operations. Written from C17 7.21 and POSIX.1-2024.
//!
//! Lock order: a thread holding REGISTRY_LOCK never waits for a stream lock (it
//! only uses try_lock), so a thread holding a stream lock may take REGISTRY_LOCK.
//! flush_all pins a stream that it must wait for and waits outside the registry
//! lock; fclose frees a stream only once no pin remains.
use super::format::{Sink, StreamOut};
use crate::{locale::MbState, pthread::RecursiveLock};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use core::{ptr, slice};

pub const EOF: i32 = -1;
/// One 4 KiB page, the kernel's page size, so a full buffer is one page-sized
/// transfer. BUFSIZ in stdio.h is this value.
const BUFFER: usize = 4096;
const EINVAL: i32 = 22;
const EBADF: i32 = 9;
const ENOMEM: i32 = 12;
const ESPIPE: i32 = 29;
const EOVERFLOW: i32 = 75;

pub(crate) struct File {
    mutex: RecursiveLock,
    fd: i32,
    readable: bool,
    writable: bool,
    append: bool,
    closed: bool,
    heap: bool,
    // Guarded by REGISTRY_LOCK.
    next: *mut File,
    linked: bool,
    pipe_child: i32,
    // flush_all's pins, which keep a closed stream's memory alive.
    pins: AtomicUsize,
    // Whether output is buffered, readable without the stream lock.
    pending: AtomicBool,
    mode: i32,
    automatic: bool,
    orientation: i32,
    state: MbState,
    eof: bool,
    error: bool,
    direction: i32,
    buffer: *mut u8,
    capacity: usize,
    owned_buffer: bool,
    embedded: [u8; BUFFER],
    read_pos: usize,
    read_end: usize,
    write_end: usize,
    pushed: [u8; 8],
    push_len: usize,
    memory: bool,
    grow: bool,
    owned_memory: bool,
    data: *mut u8,
    data_cap: usize,
    data_len: usize,
    data_pos: usize,
    publish_ptr: *mut *mut u8,
    publish_size: *mut usize,
}
// All access to shared streams and the registry is serialized by recursive locks.
unsafe impl Sync for File {}
impl File {
    const fn new(fd: i32, readable: bool, writable: bool, heap: bool) -> Self {
        Self {
            mutex: RecursiveLock::new(),
            fd,
            readable,
            writable,
            append: false,
            closed: false,
            heap,
            next: ptr::null_mut(),
            linked: false,
            pipe_child: 0,
            pins: AtomicUsize::new(0),
            pending: AtomicBool::new(false),
            mode: if fd == 2 { 2 } else { 0 },
            automatic: true,
            orientation: 0,
            state: unsafe { core::mem::zeroed() },
            eof: false,
            error: false,
            direction: 0,
            buffer: ptr::null_mut(),
            capacity: BUFFER,
            owned_buffer: false,
            embedded: [0; BUFFER],
            read_pos: 0,
            read_end: 0,
            write_end: 0,
            pushed: [0; 8],
            push_len: 0,
            memory: false,
            grow: false,
            owned_memory: false,
            data: ptr::null_mut(),
            data_cap: 0,
            data_len: 0,
            data_pos: 0,
            publish_ptr: ptr::null_mut(),
            publish_size: ptr::null_mut(),
        }
    }
}
static mut INPUT: File = File::new(0, true, false, false);
static mut OUTPUT: File = File::new(1, false, true, false);
static mut ERROR: File = File::new(2, false, true, false);
#[no_mangle]
pub static mut stdin: *mut File = ptr::addr_of_mut!(INPUT);
#[no_mangle]
pub static mut stdout: *mut File = ptr::addr_of_mut!(OUTPUT);
#[no_mangle]
pub static mut stderr: *mut File = ptr::addr_of_mut!(ERROR);
static REGISTRY_LOCK: RecursiveLock = RecursiveLock::new();
static mut HEAD: *mut File = ptr::null_mut();
static mut INITIALIZED: bool = false;
unsafe fn registry_init() {
    if !INITIALIZED {
        INPUT.next = ptr::addr_of_mut!(OUTPUT);
        OUTPUT.next = ptr::addr_of_mut!(ERROR);
        INPUT.linked = true;
        OUTPUT.linked = true;
        ERROR.linked = true;
        HEAD = ptr::addr_of_mut!(INPUT);
        INITIALIZED = true;
    }
}
pub(crate) unsafe fn lock(f: *mut File) {
    (*f).mutex.lock();
}
pub(crate) unsafe fn unlock(f: *mut File) {
    (*f).mutex.unlock();
}
pub(crate) unsafe fn orient(f: *mut File, mode: i32) -> i32 {
    if (*f).orientation == 0 && mode != 0 {
        (*f).orientation = mode.signum();
    }
    (*f).orientation
}
pub(crate) unsafe fn set_error(f: *mut File) {
    (*f).error = true;
}
// This public internal service is consumed by the locale lane's wide-stream functions.
#[allow(dead_code)]
pub(crate) unsafe fn mb_state(f: *mut File) -> *mut MbState {
    ptr::addr_of_mut!((*f).state)
}
unsafe fn fail(f: *mut File, e: i32) -> i32 {
    crate::pthread::set_errno(e);
    set_error(f);
    EOF
}
unsafe fn set_write_end(f: *mut File, n: usize) {
    (*f).write_end = n;
    (*f).pending.store(n > 0, Ordering::Release);
}
/// stdin and stdout are fully buffered only when they are not interactive
/// (C17 7.21.3p7); the first use decides.
unsafe fn resolve_mode(f: *mut File) {
    if (*f).automatic {
        (*f).automatic = false;
        if f == ptr::addr_of_mut!(INPUT) || f == ptr::addr_of_mut!(OUTPUT) {
            let saved = crate::pthread::errno();
            if crate::isatty((*f).fd) != 0 {
                (*f).mode = 1;
            }
            crate::pthread::set_errno(saved);
        }
    }
}
unsafe fn buffer(f: *mut File) -> *mut u8 {
    resolve_mode(f);
    if (*f).buffer.is_null() {
        (*f).embedded.as_mut_ptr()
    } else {
        (*f).buffer
    }
}
pub(crate) unsafe fn unbuffered(f: *mut File) -> bool {
    resolve_mode(f);
    (*f).mode == 2
}
unsafe fn allocate(fd: i32, r: bool, w: bool) -> *mut File {
    let f = crate::malloc(core::mem::size_of::<File>()) as *mut File;
    if f.is_null() {
        crate::pthread::set_errno(ENOMEM);
        return f;
    }
    ptr::write(f, File::new(fd, r, w, true));
    f
}
// Publish only fully initialized streams; fflush(NULL) can visit them immediately.
unsafe fn register(f: *mut File) {
    REGISTRY_LOCK.lock();
    registry_init();
    (*f).next = HEAD;
    (*f).linked = true;
    HEAD = f;
    REGISTRY_LOCK.unlock();
}
unsafe fn unregister(f: *mut File) {
    REGISTRY_LOCK.lock();
    registry_init();
    let mut link = ptr::addr_of_mut!(HEAD);
    while !(*link).is_null() {
        if *link == f {
            *link = (*f).next;
            break;
        }
        link = ptr::addr_of_mut!((**link).next);
    }
    (*f).linked = false;
    REGISTRY_LOCK.unlock();
}
unsafe fn mode(m: *const u8) -> Option<(i32, bool, bool, bool)> {
    if m.is_null() {
        crate::pthread::set_errno(EINVAL);
        return None;
    }
    let first = *m;
    let (mut flags, mut r, mut w, append) = match first {
        b'r' => (0, true, false, false),
        b'w' => (1 | 64 | 512, false, true, false),
        b'a' => (1 | 64 | 1024, false, true, true),
        _ => {
            crate::pthread::set_errno(EINVAL);
            return None;
        }
    };
    let mut p = m.add(1);
    let mut plus = false;
    let mut exclusive = false;
    let mut binary = false;
    let mut cloexec = false;
    while *p != 0 {
        match *p {
            b'+' if !plus => {
                plus = true;
                flags = (flags & !3) | 2;
                r = true;
                w = true;
            }
            b'b' if !binary => binary = true,
            b'x' if !exclusive && first == b'w' => {
                exclusive = true;
                flags |= 128;
            }
            b'e' if !cloexec => {
                cloexec = true;
                flags |= 0x80000;
            }
            _ => {
                crate::pthread::set_errno(EINVAL);
                return None;
            }
        }
        p = p.add(1);
    }
    Some((flags, r, w, append))
}
#[no_mangle]
pub unsafe extern "C" fn fopen(path: *const u8, m: *const u8) -> *mut File {
    let Some((flags, r, w, a)) = mode(m) else {
        return ptr::null_mut();
    };
    let fd = crate::open(path, flags, 0o666);
    if fd < 0 {
        return ptr::null_mut();
    }
    let f = allocate(fd, r, w);
    if f.is_null() {
        let e = crate::pthread::errno();
        crate::close(fd);
        crate::pthread::set_errno(e);
    } else {
        (*f).append = a;
        register(f);
    }
    f
}
#[no_mangle]
pub unsafe extern "C" fn fdopen(fd: i32, m: *const u8) -> *mut File {
    let Some((flags, r, w, a)) = mode(m) else {
        return ptr::null_mut();
    };
    let current = crate::fcntl(fd, 3, 0);
    if current < 0 {
        return ptr::null_mut();
    }
    if (r && current & 3 == 1) || (w && current & 3 == 0) {
        crate::pthread::set_errno(EINVAL);
        return ptr::null_mut();
    }
    if a && current & 1024 == 0 && crate::fcntl(fd, 4, (current | 1024) as u64) < 0 {
        return ptr::null_mut();
    }
    if flags & 0x80000 != 0 && crate::fcntl(fd, 2, 1) < 0 {
        return ptr::null_mut();
    }
    let f = allocate(fd, r, w);
    if !f.is_null() {
        (*f).append = a;
        register(f);
    }
    f
}
unsafe fn reserve(f: *mut File, need: usize) -> bool {
    if need <= (*f).data_cap {
        return true;
    }
    if !(*f).grow {
        fail(f, 28);
        return false;
    }
    let Some(cap) = need
        .checked_add(64)
        .and_then(|n| n.checked_next_power_of_two())
    else {
        fail(f, EOVERFLOW);
        return false;
    };
    let p = crate::realloc((*f).data, cap);
    if p.is_null() {
        fail(f, ENOMEM);
        return false;
    }
    (*f).data = p;
    (*f).data_cap = cap;
    true
}
unsafe fn publish(f: *mut File) {
    if (*f).grow {
        *(*f).data.add((*f).data_len) = 0;
        *(*f).publish_ptr = (*f).data;
        *(*f).publish_size = core::cmp::min((*f).data_pos, (*f).data_len);
    } else if (*f).memory && (*f).writable && (*f).data_len < (*f).data_cap {
        *(*f)
            .data
            .add(core::cmp::min((*f).data_len, (*f).data_cap - 1)) = 0;
    }
}
/// Writes the buffered output. On failure the unwritten bytes stay at the front of
/// the buffer, in order, and the error indicator is set.
unsafe fn flush_output(f: *mut File) -> i32 {
    if (*f).closed {
        return fail(f, EBADF);
    }
    if (*f).memory {
        publish(f);
        return 0;
    }
    let p = buffer(f);
    let mut done = 0;
    while done < (*f).write_end {
        let n = crate::write((*f).fd, p.add(done), (*f).write_end - done);
        if n < 0 && crate::pthread::errno() == 4 {
            continue;
        }
        if n <= 0 {
            if n == 0 {
                crate::pthread::set_errno(5);
            }
            ptr::copy(p.add(done), p, (*f).write_end - done);
            set_write_end(f, (*f).write_end - done);
            set_error(f);
            return EOF;
        }
        done += n as usize;
    }
    set_write_end(f, 0);
    0
}
unsafe fn unread(f: *mut File) -> usize {
    (*f).read_end - (*f).read_pos + (*f).push_len
}
/// Gives buffered input back to the file: the offset moves back over the unread
/// bytes (POSIX XSH 2.5.1). A file that cannot seek keeps them buffered, since
/// POSIX defines the repositioning only for files capable of seeking.
unsafe fn sync_input(f: *mut File) -> i32 {
    let unread = unread(f);
    if unread != 0 {
        if (*f).memory {
            (*f).data_pos = (*f).data_pos.saturating_sub((*f).push_len);
        } else {
            let saved = crate::pthread::errno();
            if crate::lseek((*f).fd, -(unread as i64), 1) < 0 {
                if crate::pthread::errno() == ESPIPE {
                    crate::pthread::set_errno(saved);
                    return 0;
                }
                set_error(f);
                return EOF;
            }
        }
    }
    (*f).read_pos = 0;
    (*f).read_end = 0;
    (*f).push_len = 0;
    0
}
/// Ends a run of input so the buffer can hold output; fails when read-ahead cannot
/// be given back to the file.
unsafe fn leave_input(f: *mut File) -> bool {
    if sync_input(f) < 0 {
        return false;
    }
    if unread(f) != 0 {
        fail(f, ESPIPE);
        return false;
    }
    true
}
/// Before input is read from the file on an unbuffered or line-buffered stream,
/// every line-buffered output stream is flushed (C17 7.21.3p3). A stream another
/// thread holds is mid-operation and is not waited for.
unsafe fn flush_line_buffered(except: *mut File) {
    REGISTRY_LOCK.lock();
    registry_init();
    let mut g = HEAD;
    while !g.is_null() {
        if g != except && (*g).pending.load(Ordering::Acquire) && (*g).mutex.try_lock() {
            if (*g).mode == 1 && !(*g).closed {
                flush_output(g);
            }
            unlock(g);
        }
        g = (*g).next;
    }
    REGISTRY_LOCK.unlock();
}
pub(crate) unsafe fn getc_locked(f: *mut File) -> i32 {
    if (*f).closed || !(*f).readable {
        return fail(f, EBADF);
    }
    if (*f).direction == 2 && flush_output(f) < 0 {
        return EOF;
    }
    (*f).direction = 1;
    if (*f).push_len > 0 {
        (*f).push_len -= 1;
        return (*f).pushed[(*f).push_len] as i32;
    }
    if (*f).eof {
        return EOF;
    }
    if (*f).memory {
        if (*f).data_pos >= (*f).data_len {
            (*f).eof = true;
            return EOF;
        }
        let c = *(*f).data.add((*f).data_pos);
        (*f).data_pos += 1;
        return c as i32;
    }
    if (*f).read_pos == (*f).read_end {
        let p = buffer(f);
        if (*f).mode != 0 {
            flush_line_buffered(f);
        }
        loop {
            let n = crate::read((*f).fd, p, if (*f).mode == 2 { 1 } else { (*f).capacity });
            if n < 0 && crate::pthread::errno() == 4 {
                continue;
            }
            if n < 0 {
                set_error(f);
                return EOF;
            }
            if n == 0 {
                (*f).eof = true;
                return EOF;
            }
            (*f).read_pos = 0;
            (*f).read_end = n as usize;
            break;
        }
    }
    let c = *buffer(f).add((*f).read_pos);
    (*f).read_pos += 1;
    c as i32
}
pub(crate) unsafe fn ungetc_locked(c: i32, f: *mut File) -> i32 {
    if c == EOF || (*f).push_len == 8 || (*f).closed || !(*f).readable {
        return EOF;
    }
    (*f).pushed[(*f).push_len] = c as u8;
    (*f).push_len += 1;
    (*f).eof = false;
    c as u8 as i32
}
unsafe fn memory_write(f: *mut File, b: &[u8]) -> usize {
    for (n, &c) in b.iter().enumerate() {
        if (*f).append {
            (*f).data_pos = (*f).data_len;
        }
        let Some(need) = (*f).data_pos.checked_add(if (*f).grow { 2 } else { 1 }) else {
            fail(f, EOVERFLOW);
            return n;
        };
        if !reserve(f, need) {
            return n;
        }
        while (*f).data_len < (*f).data_pos {
            *(*f).data.add((*f).data_len) = 0;
            (*f).data_len += 1;
        }
        *(*f).data.add((*f).data_pos) = c;
        (*f).data_pos += 1;
        (*f).data_len = core::cmp::max((*f).data_len, (*f).data_pos);
    }
    b.len()
}
/// Writes straight to the file; returns the bytes written.
unsafe fn write_direct(f: *mut File, b: &[u8]) -> usize {
    let mut done = 0;
    while done < b.len() {
        let n = crate::write((*f).fd, b.as_ptr().add(done), b.len() - done);
        if n < 0 && crate::pthread::errno() == 4 {
            continue;
        }
        if n <= 0 {
            if n == 0 {
                crate::pthread::set_errno(5);
            }
            set_error(f);
            break;
        }
        done += n as usize;
    }
    done
}
/// Returns how many bytes of `b` the stream accepted. The buffer never grows past its
/// capacity: a full buffer that cannot be flushed refuses the rest. Bytes whose
/// transmission the buffering mode requires now (unbuffered, or a line-buffered
/// newline) and that could not be written are taken back out of the buffer and
/// reported as not written, so a retry neither duplicates nor reorders them.
pub(crate) unsafe fn write_locked(f: *mut File, b: &[u8]) -> usize {
    if (*f).closed || !(*f).writable {
        fail(f, EBADF);
        return 0;
    }
    if (*f).direction == 1 && !leave_input(f) {
        return 0;
    }
    (*f).direction = 2;
    if (*f).memory {
        return memory_write(f, b);
    }
    let p = buffer(f);
    if (*f).mode == 2 {
        if (*f).write_end > 0 && flush_output(f) < 0 {
            return 0;
        }
        return write_direct(f, b);
    }
    if (*f).mode == 0 && (*f).write_end == 0 && b.len() >= (*f).capacity {
        return write_direct(f, b);
    }
    let mut accepted = 0;
    let mut tail = 0;
    while accepted < b.len() {
        if (*f).write_end == (*f).capacity {
            if flush_output(f) < 0 {
                return accepted;
            }
            tail = 0;
        }
        let k = ((*f).capacity - (*f).write_end).min(b.len() - accepted);
        ptr::copy_nonoverlapping(b.as_ptr().add(accepted), p.add((*f).write_end), k);
        set_write_end(f, (*f).write_end + k);
        accepted += k;
        tail += k;
    }
    if (*f).mode == 1 && b.contains(&b'\n') && flush_output(f) < 0 {
        let ours = tail.min((*f).write_end);
        set_write_end(f, (*f).write_end - ours);
        accepted -= ours;
    }
    accepted
}
pub(crate) unsafe fn putc_locked(c: u8, f: *mut File) -> i32 {
    if write_locked(f, &[c]) == 1 {
        c as i32
    } else {
        EOF
    }
}
/// fflush's work on one locked stream: output is written, and buffered input is
/// given back to a seekable file.
unsafe fn flush_one(f: *mut File) -> i32 {
    if (*f).closed {
        return 0;
    }
    if (*f).direction == 1 {
        sync_input(f)
    } else if (*f).writable {
        flush_output(f)
    } else {
        0
    }
}
/// fflush(NULL), also run by exit. A stream another thread holds is waited for only
/// when it has buffered output; one with none (a reader blocked in read) is in the
/// middle of an operation that leaves nothing to flush, and is passed over.
pub(crate) unsafe fn flush_all() -> i32 {
    let mut result = 0;
    REGISTRY_LOCK.lock();
    registry_init();
    let mut f = HEAD;
    while !f.is_null() {
        if (*f).mutex.try_lock() {
            if flush_one(f) < 0 {
                result = EOF;
            }
            unlock(f);
            f = (*f).next;
            continue;
        }
        if !(*f).pending.load(Ordering::Acquire) {
            f = (*f).next;
            continue;
        }
        (*f).pins.fetch_add(1, Ordering::AcqRel);
        REGISTRY_LOCK.unlock();
        lock(f);
        if flush_one(f) < 0 {
            result = EOF;
        }
        unlock(f);
        REGISTRY_LOCK.lock();
        // A stream closed meanwhile is no longer linked; its old successor may be
        // gone too, so the walk restarts. Flushing a stream twice is harmless.
        let next = if (*f).linked { (*f).next } else { HEAD };
        (*f).pins.fetch_sub(1, Ordering::AcqRel);
        f = next;
    }
    REGISTRY_LOCK.unlock();
    result
}
#[no_mangle]
pub unsafe extern "C" fn fflush(f: *mut File) -> i32 {
    if f.is_null() {
        return super::flush_all();
    }
    lock(f);
    let r = fflush_unlocked(f);
    unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn fflush_unlocked(f: *mut File) -> i32 {
    if f.is_null() {
        return super::flush_all();
    }
    if (*f).direction == 1 && !(*f).closed {
        sync_input(f)
    } else {
        flush_output(f)
    }
}
/// The close half of fclose and freopen: flush output, give back unread input
/// (POSIX XSH 2.5.1), close the descriptor and release owned memory.
unsafe fn close_stream(f: *mut File) -> i32 {
    if (*f).closed {
        return fail(f, EBADF);
    }
    let mut r = flush_one(f);
    if !(*f).memory && (*f).fd >= 0 && crate::close((*f).fd) < 0 {
        r = EOF
    }
    (*f).closed = true;
    (*f).fd = -1;
    if (*f).owned_buffer {
        crate::free((*f).buffer);
        (*f).buffer = ptr::null_mut();
        (*f).owned_buffer = false;
        // Only buffers larger than the embedded one are owned.
        (*f).capacity = BUFFER;
    }
    if (*f).owned_memory {
        crate::free((*f).data);
    }
    (*f).memory = false;
    (*f).grow = false;
    (*f).owned_memory = false;
    (*f).data = ptr::null_mut();
    (*f).data_cap = 0;
    (*f).data_len = 0;
    (*f).data_pos = 0;
    set_write_end(f, 0);
    r
}
#[no_mangle]
pub unsafe extern "C" fn fclose(f: *mut File) -> i32 {
    lock(f);
    let r = close_stream(f);
    let heap = (*f).heap;
    unlock(f);
    unregister(f);
    if heap {
        let saved = crate::pthread::errno();
        while (*f).pins.load(Ordering::Acquire) != 0 {
            crate::sched_yield();
        }
        crate::free(f as *mut u8);
        crate::pthread::set_errno(saved);
    }
    r
}
#[no_mangle]
pub unsafe extern "C" fn freopen(path: *const u8, m: *const u8, f: *mut File) -> *mut File {
    let Some((flags, r, w, a)) = mode(m) else {
        return ptr::null_mut();
    };
    lock(f);
    let reopened = if path.is_null() {
        change_mode(f, flags, r, w)
    } else {
        reopen(f, path, flags)
    };
    if !reopened {
        // C17 7.21.5.4: the stream's file is closed whether or not the open works.
        let e = crate::pthread::errno();
        if !(*f).closed {
            close_stream(f);
        }
        crate::pthread::set_errno(e);
        unlock(f);
        return ptr::null_mut();
    }
    (*f).closed = false;
    (*f).readable = r;
    (*f).writable = w;
    (*f).append = a;
    (*f).orientation = 0;
    (*f).state = core::mem::zeroed();
    (*f).eof = false;
    (*f).error = false;
    (*f).read_pos = 0;
    (*f).read_end = 0;
    set_write_end(f, 0);
    (*f).push_len = 0;
    (*f).direction = 0;
    unlock(f);
    f
}
/// freopen(NULL, mode, f): the descriptor stays and must allow the new access mode
/// (POSIX freopen, EBADF); only O_APPEND and FD_CLOEXEC change.
unsafe fn change_mode(f: *mut File, flags: i32, r: bool, w: bool) -> bool {
    if (*f).closed || (*f).memory {
        crate::pthread::set_errno(EBADF);
        return false;
    }
    if flush_one(f) < 0 {
        return false;
    }
    let fd = (*f).fd;
    let current = crate::fcntl(fd, 3, 0);
    if current < 0 {
        return false;
    }
    if (r && current & 3 == 1) || (w && current & 3 == 0) {
        crate::pthread::set_errno(EBADF);
        return false;
    }
    if crate::fcntl(fd, 4, ((current & !1024) | (flags & 1024)) as u64) < 0 {
        return false;
    }
    if crate::fcntl(fd, 2, if flags & 0x80000 != 0 { 1 } else { 0 }) < 0 {
        return false;
    }
    (*f).read_pos = 0;
    (*f).read_end = 0;
    (*f).push_len = 0;
    true
}
/// freopen(path, mode, f): close the old file, then open path on the same
/// descriptor number when the stream had one.
unsafe fn reopen(f: *mut File, path: *const u8, flags: i32) -> bool {
    let old = (*f).fd;
    let had_fd = !(*f).closed && !(*f).memory && old >= 0;
    if !(*f).closed {
        close_stream(f);
    }
    let fd = crate::open(path, flags, 0o666);
    if fd < 0 {
        return false;
    }
    (*f).fd = fd;
    if had_fd && fd != old {
        if crate::dup2(fd, old) < 0 {
            let e = crate::pthread::errno();
            crate::close(fd);
            (*f).fd = -1;
            crate::pthread::set_errno(e);
            return false;
        }
        crate::close(fd);
        (*f).fd = old;
        if flags & 0x80000 != 0 {
            crate::fcntl(old, 2, 1);
        }
    }
    true
}
#[no_mangle]
pub unsafe extern "C" fn setvbuf(f: *mut File, b: *mut u8, m: i32, size: usize) -> i32 {
    if !(0..=2).contains(&m) {
        crate::pthread::set_errno(EINVAL);
        return -1;
    }
    lock(f);
    // The request cannot be honored when output cannot be written, or when read-ahead
    // that an unseekable file cannot take back would be lost with the old buffer.
    if fflush_unlocked(f) < 0 || ((*f).read_end > (*f).read_pos) {
        unlock(f);
        return -1;
    }
    let (p, cap, own) = if m == 2 {
        (ptr::null_mut(), 1, false)
    } else if !b.is_null() && size > 0 {
        (b, size, false)
    } else {
        // No caller array (or one of size 0): the stream's own buffer.
        let cap = if size == 0 { BUFFER } else { size };
        if cap <= BUFFER {
            (ptr::null_mut(), cap, false)
        } else {
            let p = crate::malloc(cap);
            if p.is_null() {
                crate::pthread::set_errno(ENOMEM);
                unlock(f);
                return -1;
            }
            (p, cap, true)
        }
    };
    if (*f).owned_buffer {
        crate::free((*f).buffer);
    }
    (*f).buffer = p;
    (*f).capacity = cap;
    (*f).owned_buffer = own;
    (*f).mode = m;
    (*f).automatic = false;
    unlock(f);
    0
}
#[no_mangle]
pub unsafe extern "C" fn setbuf(f: *mut File, b: *mut u8) {
    setvbuf(f, b, if b.is_null() { 2 } else { 0 }, BUFFER);
}
#[no_mangle]
pub unsafe extern "C" fn fgetc_unlocked(f: *mut File) -> i32 {
    orient(f, -1);
    getc_locked(f)
}
#[no_mangle]
pub unsafe extern "C" fn fgetc(f: *mut File) -> i32 {
    lock(f);
    let r = fgetc_unlocked(f);
    unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn getc(f: *mut File) -> i32 {
    fgetc(f)
}
#[no_mangle]
pub unsafe extern "C" fn getc_unlocked(f: *mut File) -> i32 {
    fgetc_unlocked(f)
}
#[no_mangle]
pub unsafe extern "C" fn getchar() -> i32 {
    fgetc(stdin)
}
#[no_mangle]
pub unsafe extern "C" fn getchar_unlocked() -> i32 {
    fgetc_unlocked(stdin)
}
#[no_mangle]
pub unsafe extern "C" fn fputc_unlocked(c: i32, f: *mut File) -> i32 {
    orient(f, -1);
    putc_locked(c as u8, f)
}
#[no_mangle]
pub unsafe extern "C" fn fputc(c: i32, f: *mut File) -> i32 {
    lock(f);
    let r = fputc_unlocked(c, f);
    unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn putc(c: i32, f: *mut File) -> i32 {
    fputc(c, f)
}
#[no_mangle]
pub unsafe extern "C" fn putc_unlocked(c: i32, f: *mut File) -> i32 {
    fputc_unlocked(c, f)
}
#[no_mangle]
pub unsafe extern "C" fn putchar(c: i32) -> i32 {
    fputc(c, stdout)
}
#[no_mangle]
pub unsafe extern "C" fn putchar_unlocked(c: i32) -> i32 {
    fputc_unlocked(c, stdout)
}
#[no_mangle]
pub unsafe extern "C" fn ungetc(c: i32, f: *mut File) -> i32 {
    lock(f);
    orient(f, -1);
    let r = ungetc_locked(c, f);
    unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn fread_unlocked(p: *mut u8, size: usize, n: usize, f: *mut File) -> usize {
    let Some(total) = size.checked_mul(n) else {
        fail(f, EOVERFLOW);
        return 0;
    };
    if total == 0 {
        return 0;
    }
    orient(f, -1);
    let mut done = 0;
    while done < total {
        let c = getc_locked(f);
        if c == EOF {
            break;
        }
        *p.add(done) = c as u8;
        done += 1;
    }
    done / size
}
#[no_mangle]
pub unsafe extern "C" fn fread(p: *mut u8, s: usize, n: usize, f: *mut File) -> usize {
    lock(f);
    let r = fread_unlocked(p, s, n, f);
    unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn fwrite_unlocked(
    p: *const u8,
    size: usize,
    n: usize,
    f: *mut File,
) -> usize {
    let Some(total) = size.checked_mul(n) else {
        fail(f, EOVERFLOW);
        return 0;
    };
    if total == 0 {
        return 0;
    }
    orient(f, -1);
    write_locked(f, slice::from_raw_parts(p, total)) / size
}
#[no_mangle]
pub unsafe extern "C" fn fwrite(p: *const u8, s: usize, n: usize, f: *mut File) -> usize {
    lock(f);
    let r = fwrite_unlocked(p, s, n, f);
    unlock(f);
    r
}
unsafe fn c_str<'a>(s: *const u8) -> &'a [u8] {
    let mut n = 0;
    while *s.add(n) != 0 {
        n += 1;
    }
    slice::from_raw_parts(s, n)
}
#[no_mangle]
pub unsafe extern "C" fn fputs_unlocked(s: *const u8, f: *mut File) -> i32 {
    orient(f, -1);
    let s = c_str(s);
    if write_locked(f, s) == s.len() {
        0
    } else {
        EOF
    }
}
#[no_mangle]
pub unsafe extern "C" fn fputs(s: *const u8, f: *mut File) -> i32 {
    lock(f);
    let r = fputs_unlocked(s, f);
    unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn puts(s: *const u8) -> i32 {
    let f = stdout;
    lock(f);
    orient(f, -1);
    let mut out = StreamOut::new(f);
    let ok = out.bytes(c_str(s)) && out.bytes(b"\n") && out.finish();
    unlock(f);
    if ok {
        0
    } else {
        EOF
    }
}
#[no_mangle]
pub unsafe extern "C" fn fgets_unlocked(p: *mut u8, n: i32, f: *mut File) -> *mut u8 {
    if n <= 0 {
        return ptr::null_mut();
    }
    orient(f, -1);
    let mut i = 0;
    let mut failed = false;
    while i < (n - 1) as usize {
        let c = getc_locked(f);
        if c == EOF {
            // An EOF return without the end-of-file indicator is this call's read
            // error; an error indicator left by an earlier call does not count.
            failed = !(*f).eof;
            break;
        }
        *p.add(i) = c as u8;
        i += 1;
        if c == 10 {
            break;
        }
    }
    if failed || (i == 0 && n > 1) {
        ptr::null_mut()
    } else {
        *p.add(i) = 0;
        p
    }
}
#[no_mangle]
pub unsafe extern "C" fn fgets(p: *mut u8, n: i32, f: *mut File) -> *mut u8 {
    lock(f);
    let r = fgets_unlocked(p, n, f);
    unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn feof_unlocked(f: *mut File) -> i32 {
    (*f).eof as i32
}
#[no_mangle]
pub unsafe extern "C" fn ferror_unlocked(f: *mut File) -> i32 {
    (*f).error as i32
}
#[no_mangle]
pub unsafe extern "C" fn clearerr_unlocked(f: *mut File) {
    (*f).eof = false;
    (*f).error = false;
}
#[no_mangle]
pub unsafe extern "C" fn feof(f: *mut File) -> i32 {
    lock(f);
    let r = feof_unlocked(f);
    unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn ferror(f: *mut File) -> i32 {
    lock(f);
    let r = ferror_unlocked(f);
    unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn clearerr(f: *mut File) {
    lock(f);
    clearerr_unlocked(f);
    unlock(f);
}
#[no_mangle]
pub unsafe extern "C" fn fileno_unlocked(f: *mut File) -> i32 {
    if (*f).memory || (*f).closed {
        fail(f, EBADF)
    } else {
        (*f).fd
    }
}
#[no_mangle]
pub unsafe extern "C" fn fileno(f: *mut File) -> i32 {
    lock(f);
    let r = fileno_unlocked(f);
    unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn flockfile(f: *mut File) {
    lock(f)
}
#[no_mangle]
pub unsafe extern "C" fn ftrylockfile(f: *mut File) -> i32 {
    if (*f).mutex.try_lock() {
        0
    } else {
        1
    }
}
#[no_mangle]
pub unsafe extern "C" fn funlockfile(f: *mut File) {
    unlock(f)
}
unsafe fn tell(f: *mut File) -> i64 {
    if (*f).memory {
        return (*f).data_pos as i64 - (*f).push_len as i64;
    }
    if (*f).append && (*f).write_end > 0 {
        // Buffered append output lands at the end of the file when it is written.
        let end = crate::lseek((*f).fd, 0, 2);
        if end < 0 {
            return -1;
        }
        return end + (*f).write_end as i64;
    }
    let p = crate::lseek((*f).fd, 0, 1);
    if p < 0 {
        return -1;
    }
    p + (*f).write_end as i64 - unread(f) as i64
}
#[no_mangle]
pub unsafe extern "C" fn ftello(f: *mut File) -> i64 {
    lock(f);
    let r = tell(f);
    unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn ftell(f: *mut File) -> i64 {
    ftello(f)
}
#[no_mangle]
pub unsafe extern "C" fn fseeko(f: *mut File, offset: i64, whence: i32) -> i32 {
    lock(f);
    let r = seek(f, offset, whence);
    unlock(f);
    r
}
unsafe fn seek(f: *mut File, offset: i64, whence: i32) -> i32 {
    if !(0..=2).contains(&whence) {
        crate::pthread::set_errno(EINVAL);
        return -1;
    }
    if flush_output(f) < 0 {
        return -1;
    }
    if (*f).memory {
        let base = match whence {
            0 => 0,
            1 => (*f).data_pos as i64 - (*f).push_len as i64,
            _ => (*f).data_len as i64,
        };
        let Some(pos) = base.checked_add(offset).filter(|p| *p >= 0) else {
            crate::pthread::set_errno(EINVAL);
            return -1;
        };
        if !(*f).grow && pos as usize > (*f).data_cap {
            crate::pthread::set_errno(EINVAL);
            return -1;
        }
        (*f).data_pos = pos as usize;
    } else {
        let adj = if whence == 1 {
            offset.checked_sub(unread(f) as i64)
        } else {
            Some(offset)
        };
        let Some(adj) = adj else {
            crate::pthread::set_errno(EOVERFLOW);
            return -1;
        };
        if crate::lseek((*f).fd, adj, whence) < 0 {
            return -1;
        }
    }
    (*f).read_pos = 0;
    (*f).read_end = 0;
    (*f).push_len = 0;
    (*f).eof = false;
    (*f).direction = 0;
    0
}
#[no_mangle]
pub unsafe extern "C" fn fseek(f: *mut File, o: i64, w: i32) -> i32 {
    fseeko(f, o, w)
}
#[no_mangle]
pub unsafe extern "C" fn rewind(f: *mut File) {
    lock(f);
    seek(f, 0, 0);
    (*f).error = false;
    unlock(f);
}
#[repr(C)]
pub struct Position {
    offset: i64,
    state: [u8; 8],
}
#[no_mangle]
pub unsafe extern "C" fn fgetpos(f: *mut File, p: *mut Position) -> i32 {
    lock(f);
    let n = tell(f);
    let r = if n < 0 {
        -1
    } else {
        (*p).offset = n;
        ptr::copy_nonoverlapping(
            ptr::addr_of!((*f).state) as *const u8,
            (*p).state.as_mut_ptr(),
            8,
        );
        0
    };
    unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn fsetpos(f: *mut File, p: *const Position) -> i32 {
    lock(f);
    let r = seek(f, (*p).offset, 0);
    if r == 0 {
        ptr::copy_nonoverlapping(
            (*p).state.as_ptr(),
            ptr::addr_of_mut!((*f).state) as *mut u8,
            8,
        );
    }
    unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn getdelim(
    line: *mut *mut u8,
    cap: *mut usize,
    delimiter: i32,
    f: *mut File,
) -> isize {
    if line.is_null() || cap.is_null() {
        crate::pthread::set_errno(EINVAL);
        return -1;
    }
    lock(f);
    orient(f, -1);
    let mut n = 0;
    let mut r = -1;
    loop {
        if (*line).is_null() || n + 1 >= *cap {
            let Some(newcap) =
                core::cmp::max(if (*line).is_null() { 0 } else { *cap }, 64).checked_mul(2)
            else {
                fail(f, EOVERFLOW);
                break;
            };
            let p = crate::realloc(*line, newcap);
            if p.is_null() {
                fail(f, ENOMEM);
                break;
            }
            *line = p;
            *cap = newcap;
        }
        let c = getc_locked(f);
        if c == EOF {
            // End of file after some bytes is a complete last line; an EOF return
            // without the end-of-file indicator is this call's read error.
            if n > 0 && (*f).eof {
                r = n as isize;
            }
            break;
        }
        *(*line).add(n) = c as u8;
        n += 1;
        *(*line).add(n) = 0;
        if c == delimiter as u8 as i32 {
            r = n as isize;
            break;
        }
        if n == isize::MAX as usize {
            fail(f, EOVERFLOW);
            break;
        }
    }
    unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn getline(l: *mut *mut u8, n: *mut usize, f: *mut File) -> isize {
    getdelim(l, n, 10, f)
}
#[no_mangle]
pub unsafe extern "C" fn open_memstream(p: *mut *mut u8, n: *mut usize) -> *mut File {
    if p.is_null() || n.is_null() {
        crate::pthread::set_errno(EINVAL);
        return ptr::null_mut();
    }
    let f = allocate(-1, false, true);
    if f.is_null() {
        return f;
    }
    (*f).memory = true;
    (*f).grow = true;
    (*f).publish_ptr = p;
    (*f).publish_size = n;
    if !reserve(f, 1) {
        crate::free(f as *mut u8);
        return ptr::null_mut();
    }
    *(*f).data = 0;
    publish(f);
    register(f);
    f
}
#[no_mangle]
pub unsafe extern "C" fn fmemopen(p: *mut u8, n: usize, m: *const u8) -> *mut File {
    let Some((_, r, w, a)) = mode(m) else {
        return ptr::null_mut();
    };
    let f = allocate(-1, r, w);
    if f.is_null() {
        return f;
    }
    (*f).memory = true;
    (*f).data_cap = n;
    (*f).append = a;
    (*f).data = p;
    if p.is_null() {
        (*f).data = crate::calloc(core::cmp::max(n, 1), 1);
        (*f).owned_memory = true;
        if (*f).data.is_null() {
            crate::pthread::set_errno(ENOMEM);
            crate::free(f as *mut u8);
            return ptr::null_mut();
        }
    }
    (*f).data_len = if *m == b'w' {
        if n > 0 {
            *(*f).data = 0;
        }
        0
    } else if a {
        let mut k = 0;
        while k < n && *(*f).data.add(k) != 0 {
            k += 1;
        }
        k
    } else {
        n
    };
    // POSIX fmemopen: append mode starts at the first null byte, or at size when
    // the buffer has none.
    (*f).data_pos = if a { (*f).data_len } else { 0 };
    register(f);
    f
}
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
/// Attempts before tmpfile reports EEXIST. Each candidate carries 32 random bits, so
/// a collision needs an existing file with that exact name; 64 consecutive ones
/// means something is creating the names deliberately.
const TEMP_ATTEMPTS: usize = 64;
#[no_mangle]
pub unsafe extern "C" fn tmpfile() -> *mut File {
    let mut path = [0u8; 64];
    let prefix = b"/tmp/breenix-";
    path[..prefix.len()].copy_from_slice(prefix);
    for _ in 0..TEMP_ATTEMPTS {
        super::misc::temp_name(&mut path[prefix.len()..], &TEMP_SEQUENCE);
        let fd = crate::open(path.as_ptr(), 2 | 64 | 128, 0o600);
        if fd < 0 {
            if crate::pthread::errno() == 17 {
                continue;
            }
            return ptr::null_mut();
        }
        if crate::unlink(path.as_ptr()) < 0 {
            let e = crate::pthread::errno();
            crate::close(fd);
            crate::pthread::set_errno(e);
            return ptr::null_mut();
        }
        let f = allocate(fd, true, true);
        if f.is_null() {
            let e = crate::pthread::errno();
            crate::close(fd);
            crate::pthread::set_errno(e);
        } else {
            register(f);
        }
        return f;
    }
    crate::pthread::set_errno(17);
    ptr::null_mut()
}

/// Marks f as a popen stream, makes its descriptor inheritable unless the caller
/// asked for close-on-exec, and forks, all under the registry lock. Every popen
/// forks under that lock, so each child sees the streams of earlier popen calls
/// marked and closes them (POSIX popen); until the mark is set the descriptor is
/// close-on-exec and cannot reach another popen's command.
/// The child only closes raw descriptors and execs/_exits: it never uses the copied lock.
pub(super) unsafe fn fork_pipe(f: *mut File, cloexec: bool) -> i32 {
    REGISTRY_LOCK.lock();
    registry_init();
    (*f).pipe_child = -1;
    if !cloexec && crate::fcntl((*f).fd, 2, 0) < 0 {
        (*f).pipe_child = 0;
        REGISTRY_LOCK.unlock();
        return -1;
    }
    let pid = crate::fork();
    if pid != 0 {
        (*f).pipe_child = pid.max(0);
        REGISTRY_LOCK.unlock();
    }
    pid
}
pub(super) unsafe fn close_inherited_pipes(current: *mut File) {
    let mut f = HEAD;
    while !f.is_null() {
        if f != current && (*f).pipe_child != 0 {
            crate::close((*f).fd);
        }
        f = (*f).next;
    }
}
pub(super) unsafe fn take_pipe_pid(f: *mut File) -> i32 {
    REGISTRY_LOCK.lock();
    let pid = (*f).pipe_child;
    (*f).pipe_child = 0;
    REGISTRY_LOCK.unlock();
    pid
}
