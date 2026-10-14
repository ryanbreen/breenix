//! Stream state and shared locked operations. Written from C17 7.21 and POSIX.1-2024.
use crate::{locale::MbState, pthread::RecursiveLock};
use core::{ptr, slice};

pub const EOF: i32 = -1;
const BUFFER: usize = 1024;
const EINVAL: i32 = 22;
const EBADF: i32 = 9;
const ENOMEM: i32 = 12;
const EOVERFLOW: i32 = 75;

pub(crate) struct File {
    mutex: RecursiveLock,
    fd: i32,
    readable: bool,
    writable: bool,
    append: bool,
    closed: bool,
    heap: bool,
    next: *mut File,
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
unsafe fn buffer(f: *mut File) -> *mut u8 {
    if (*f).automatic {
        let saved = crate::pthread::errno();
        if (*f).fd == 1 && crate::isatty((*f).fd) != 0 {
            (*f).mode = 1;
        }
        crate::pthread::set_errno(saved);
        (*f).automatic = false;
    }
    if (*f).buffer.is_null() {
        (*f).embedded.as_mut_ptr()
    } else {
        (*f).buffer
    }
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
    HEAD = f;
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
            (*f).write_end -= done;
            set_error(f);
            return EOF;
        }
        done += n as usize;
    }
    (*f).write_end = 0;
    0
}
unsafe fn sync_input(f: *mut File) -> i32 {
    let unread = (*f).read_end - (*f).read_pos + (*f).push_len;
    if unread != 0 {
        if (*f).memory {
            (*f).data_pos = (*f).data_pos.saturating_sub((*f).push_len);
        } else if crate::lseek((*f).fd, -(unread as i64), 1) < 0 {
            set_error(f);
            return EOF;
        }
    }
    (*f).read_pos = 0;
    (*f).read_end = 0;
    (*f).push_len = 0;
    0
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
pub(crate) unsafe fn putc_locked(c: u8, f: *mut File) -> i32 {
    if (*f).closed || !(*f).writable {
        return fail(f, EBADF);
    }
    if (*f).direction == 1 && sync_input(f) < 0 {
        return EOF;
    }
    (*f).direction = 2;
    if (*f).memory {
        if (*f).append {
            (*f).data_pos = (*f).data_len;
        }
        let Some(need) = (*f).data_pos.checked_add(if (*f).grow { 2 } else { 1 }) else {
            return fail(f, EOVERFLOW);
        };
        if !reserve(f, need) {
            return EOF;
        }
        while (*f).data_len < (*f).data_pos {
            *(*f).data.add((*f).data_len) = 0;
            (*f).data_len += 1;
        }
        *(*f).data.add((*f).data_pos) = c;
        (*f).data_pos += 1;
        (*f).data_len = core::cmp::max((*f).data_len, (*f).data_pos);
        return c as i32;
    }
    let p = buffer(f);
    *p.add((*f).write_end) = c;
    (*f).write_end += 1;
    if ((*f).mode == 2 || (*f).write_end == (*f).capacity || ((*f).mode == 1 && c == b'\n'))
        && flush_output(f) < 0
    {
        return EOF;
    }
    c as i32
}
pub(crate) unsafe fn write_locked(f: *mut File, b: &[u8]) -> usize {
    let mut n = 0;
    for &c in b {
        if putc_locked(c, f) == EOF {
            break;
        }
        n += 1;
    }
    n
}
pub(crate) unsafe fn flush_all() -> i32 {
    REGISTRY_LOCK.lock();
    registry_init();
    let mut f = HEAD;
    let mut result = 0;
    while !f.is_null() {
        lock(f);
        if (*f).writable && flush_output(f) < 0 {
            result = EOF
        }
        unlock(f);
        f = (*f).next;
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
    if (*f).direction == 1 {
        sync_input(f)
    } else {
        flush_output(f)
    }
}
#[no_mangle]
pub unsafe extern "C" fn fclose(f: *mut File) -> i32 {
    REGISTRY_LOCK.lock();
    registry_init();
    lock(f);
    let mut r = flush_output(f);
    if !(*f).memory && crate::close((*f).fd) < 0 {
        r = EOF
    }
    (*f).closed = true;
    let mut link = ptr::addr_of_mut!(HEAD);
    while !(*link).is_null() {
        if *link == f {
            *link = (*f).next;
            break;
        }
        link = ptr::addr_of_mut!((**link).next);
    }
    if (*f).owned_buffer {
        crate::free((*f).buffer);
    }
    if (*f).owned_memory {
        crate::free((*f).data);
    }
    let heap = (*f).heap;
    unlock(f);
    if heap {
        crate::free(f as *mut u8);
    }
    REGISTRY_LOCK.unlock();
    r
}
#[no_mangle]
pub unsafe extern "C" fn freopen(path: *const u8, m: *const u8, f: *mut File) -> *mut File {
    let Some((flags, r, w, a)) = mode(m) else {
        return ptr::null_mut();
    };
    lock(f);
    flush_output(f);
    let old = (*f).fd;
    if !path.is_null() {
        crate::close(old);
        (*f).closed = true;
        (*f).fd = crate::open(path, flags, 0o666);
    } else {
        let fl = crate::fcntl(old, 3, 0);
        if fl < 0 || crate::fcntl(old, 4, ((fl & !1024) | (flags & 1024)) as u64) < 0 {
            unlock(f);
            return ptr::null_mut();
        }
    }
    if (*f).fd < 0 {
        unlock(f);
        return ptr::null_mut();
    }
    if (*f).fd != old && old >= 0 {
        if crate::dup2((*f).fd, old) < 0 {
            crate::close((*f).fd);
            unlock(f);
            return ptr::null_mut();
        }
        crate::close((*f).fd);
        (*f).fd = old;
        if flags & 0x80000 != 0 {
            crate::fcntl(old, 2, 1);
        }
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
    (*f).write_end = 0;
    (*f).push_len = 0;
    (*f).direction = 0;
    unlock(f);
    f
}
#[no_mangle]
pub unsafe extern "C" fn setvbuf(f: *mut File, b: *mut u8, m: i32, size: usize) -> i32 {
    if !(0..=2).contains(&m) {
        crate::pthread::set_errno(EINVAL);
        return -1;
    }
    lock(f);
    if fflush_unlocked(f) < 0 {
        unlock(f);
        return -1;
    }
    let cap = if m == 2 {
        1
    } else if size == 0 {
        BUFFER
    } else {
        size
    };
    let mut p = b;
    let own = p.is_null() && cap > BUFFER;
    if own {
        p = crate::malloc(cap);
        if p.is_null() {
            crate::pthread::set_errno(ENOMEM);
            unlock(f);
            return -1;
        }
    }
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
    if size == 0 {
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
    if size == 0 {
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
#[no_mangle]
pub unsafe extern "C" fn fputs(s: *const u8, f: *mut File) -> i32 {
    lock(f);
    orient(f, -1);
    let mut p = s;
    let mut r = 0;
    while *p != 0 {
        if putc_locked(*p, f) == EOF {
            r = EOF;
            break;
        }
        p = p.add(1);
    }
    unlock(f);
    r
}
#[no_mangle]
pub unsafe extern "C" fn puts(s: *const u8) -> i32 {
    lock(stdout);
    let r = fputs(s, stdout);
    let n = fputc(b'\n' as i32, stdout);
    unlock(stdout);
    if r < 0 || n < 0 {
        EOF
    } else {
        0
    }
}
#[no_mangle]
pub unsafe extern "C" fn fgets(p: *mut u8, n: i32, f: *mut File) -> *mut u8 {
    if n <= 0 {
        return ptr::null_mut();
    }
    lock(f);
    orient(f, -1);
    let mut i = 0;
    while i < (n - 1) as usize {
        let c = getc_locked(f);
        if c == EOF {
            break;
        }
        *p.add(i) = c as u8;
        i += 1;
        if c == 10 {
            break;
        }
    }
    let r = if (*f).error || (i == 0 && n > 1) {
        ptr::null_mut()
    } else {
        *p.add(i) = 0;
        p
    };
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
pub unsafe extern "C" fn fileno(f: *mut File) -> i32 {
    lock(f);
    let r = if (*f).memory || (*f).closed {
        fail(f, EBADF)
    } else {
        (*f).fd
    };
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
    let p = crate::lseek((*f).fd, 0, 1);
    if p < 0 {
        return -1;
    }
    p + (*f).write_end as i64 - ((*f).read_end - (*f).read_pos + (*f).push_len) as i64
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
            offset.checked_sub(((*f).read_end - (*f).read_pos + (*f).push_len) as i64)
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
            let Some(newcap) = core::cmp::max(*cap, 64).checked_mul(2) else {
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
            if n > 0 && !(*f).error {
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
    (*f).data_pos = if a && !p.is_null() {
        if (*f).data_len == n {
            n.saturating_add(1)
        } else {
            (*f).data_len
        }
    } else {
        0
    };
    register(f);
    f
}
use core::sync::atomic::{AtomicU64, Ordering};
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[no_mangle]
pub unsafe extern "C" fn tmpfile() -> *mut File {
    let mut path = [0u8; 64];
    let prefix = b"/tmp/breenix-";
    path[..prefix.len()].copy_from_slice(prefix);
    for _ in 0..238328 {
        let token =
            ((crate::getpid() as u64) << 32) ^ TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        for i in 0..16 {
            path[prefix.len() + i] = b"0123456789abcdef"[((token >> ((15 - i) * 4)) & 15) as usize];
        }
        let fd = crate::open(path.as_ptr(), 2 | 64 | 128, 0o600);
        if fd < 0 {
            if crate::pthread::errno() == 17 {
                continue;
            }
            return ptr::null_mut();
        }
        if crate::unlink(path.as_ptr()) < 0 {
            crate::close(fd);
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
