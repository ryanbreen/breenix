//! Clean-room POSIX threads. Object layouts fit the Linux ABI storage used by
//! Breenix's C clients; zero is the static initializer for every lock and once.
//! All sleeping uses futexes. Kernel clear_child_tid is the reclamation fence:
//! publishing a result alone never permits a stack or descriptor to be unmapped.
use super::{EBUSY, EINVAL, ENOMEM, EPERM};
use core::ptr::{null, null_mut};
use core::sync::atomic::{
    AtomicBool, AtomicU32,
    Ordering::{Acquire, Relaxed, Release},
};
use libbreenix::syscall::{nr, raw};
const ESRCH: i32 = 3;
const EAGAIN: i32 = 11;
const EDEADLK: i32 = 35;
const ETIMEDOUT: i32 = 110;
const EOWNERDEAD: i32 = 130;
const ENOTRECOVERABLE: i32 = 131;
const PAGE: usize = 4096;
const KEYS: usize = 128;
const STACK_MIN: usize = 16384;
type Start = extern "C" fn(*mut u8) -> *mut u8;
type Destructor = Option<unsafe extern "C" fn(*mut u8)>;

unsafe fn word<'a>(p: *const u32) -> &'a AtomicU32 {
    &*(p as *const AtomicU32)
}
unsafe fn futex(p: *const AtomicU32, op: u64, v: u32, timeout: *const i64) -> i32 {
    raw::syscall6(
        nr::FUTEX,
        p as u64,
        op,
        v as u64,
        timeout as u64,
        0,
        u32::MAX as u64,
    ) as i64 as i32
}
unsafe fn wake(p: &AtomicU32, n: u32) {
    futex(p, 1, n, null());
}
unsafe fn wait(p: &AtomicU32, v: u32) {
    futex(p, 0, v, null());
}

/// Internal locks use the same futex protocol as public locks, without errno or
/// descriptor lookup, so allocation of the first descriptor cannot recurse.
struct Lock(AtomicU32);
impl Lock {
    const fn new() -> Self {
        Self(AtomicU32::new(0))
    }
    fn try_lock(&self) -> bool {
        self.0.compare_exchange(0, 1, Acquire, Relaxed).is_ok()
    }
    fn lock(&self) {
        if self.0.compare_exchange(0, 1, Acquire, Relaxed).is_ok() {
            return;
        }
        while self.0.swap(2, Acquire) != 0 {
            unsafe {
                wait(&self.0, 2);
            }
        }
    }
    fn unlock(&self) {
        if self.0.swap(0, Release) == 2 {
            unsafe {
                wake(&self.0, 1);
            }
        }
    }
}

unsafe fn map(len: usize, prot: u64) -> *mut u8 {
    let r = raw::syscall6(nr::MMAP, 0, len as u64, prot, 0x22, u64::MAX, 0) as i64;
    if r < 0 {
        null_mut()
    } else {
        r as *mut u8
    }
}
unsafe fn unmap(p: *mut u8, len: usize) {
    if !p.is_null() {
        raw::syscall2(nr::MUNMAP, p as u64, len as u64);
    }
}
fn tid() -> u32 {
    unsafe { raw::syscall0(nr::GETTID) as u32 }
}
fn pid() -> u32 {
    unsafe { raw::syscall0(nr::GETPID) as u32 }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Attr {
    stack: *mut u8,
    size: usize,
    guard: usize,
    detached: i32,
    inherit: i32,
    policy: i32,
    priority: i32,
}
impl Attr {
    const fn new() -> Self {
        Self {
            stack: null_mut(),
            size: 2 * 1024 * 1024,
            guard: PAGE,
            detached: 0,
            inherit: 0,
            policy: 0,
            priority: 0,
        }
    }
}
#[repr(C)]
struct Thread {
    next: *mut Thread,
    id: u32,
    clear: AtomicU32,
    // 0 joinable, 1 detached, 2 join claimed, 3 reaper claimed.
    // Only the registry owns reclamation.
    state: u32,
    ready: AtomicU32,
    exiting: bool,
    result: *mut u8,
    start: Option<Start>,
    arg: *mut u8,
    attr: Attr,
    mapping: *mut u8,
    mapping_len: usize,
    tls: *mut u8,
    tls_len: usize,
    tp: usize,
    errno: i32,
    robust: *mut Mutex,
    values: [(*mut u8, u32); KEYS],
    extra_values: *mut Values,
    reads: *mut ReadBlock,
}
static REGISTRY: Lock = Lock::new();
static PROCESS: AtomicU32 = AtomicU32::new(0);
static LIVE: AtomicU32 = AtomicU32::new(0);
static RUNTIME_READY: AtomicBool = AtomicBool::new(false);
static REAPER_EVENT: AtomicU32 = AtomicU32::new(0);
static mut REAPER_STARTED: bool = false;
static mut HEAD: *mut Thread = null_mut();
static mut TLS_IMAGE: *const u8 = null();
static mut TLS_FILE: usize = 0;
static mut TLS_SIZE: usize = 0;
static mut TLS_ALIGN: usize = 16;
fn rounded(n: usize, a: usize) -> Option<usize> {
    n.checked_add(a - 1).map(|v| v & !(a - 1))
}

unsafe fn alloc_thread() -> *mut Thread {
    let p = map(rounded(core::mem::size_of::<Thread>(), PAGE).unwrap(), 3) as *mut Thread;
    if p.is_null() {
        return p;
    }
    core::ptr::write(
        p,
        Thread {
            next: null_mut(),
            id: 0,
            clear: AtomicU32::new(0),
            state: 0,
            ready: AtomicU32::new(0),
            exiting: false,
            result: null_mut(),
            start: None,
            arg: null_mut(),
            attr: Attr::new(),
            mapping: null_mut(),
            mapping_len: 0,
            tls: null_mut(),
            tls_len: 0,
            tp: 0,
            errno: 0,
            robust: null_mut(),
            values: [(null_mut(), 0); KEYS],
            extra_values: null_mut(),
            reads: null_mut(),
        },
    );
    let align = TLS_ALIGN.max(16);
    let Some(len) = TLS_SIZE
        .checked_add(align * 2)
        .and_then(|v| v.checked_add(32))
        .and_then(|v| rounded(v, PAGE))
    else {
        free_thread(p);
        return null_mut();
    };
    let block = map(len, 3);
    if block.is_null() {
        free_thread(p);
        return null_mut();
    }
    (*p).tls = block;
    (*p).tls_len = len;
    #[cfg(target_arch = "aarch64")]
    let (tp, data) = {
        // AArch64 TLS variant I: two reserved words before aligned static TLS.
        let tp = rounded(block as usize, align).unwrap();
        (tp, rounded(tp + 16, align).unwrap())
    };
    #[cfg(target_arch = "x86_64")]
    let (tp, data) = {
        // x86 TLS variant II: static TLS immediately precedes the thread pointer.
        let data = rounded(block as usize, align).unwrap();
        (data + rounded(TLS_SIZE, align).unwrap(), data)
    };
    if TLS_FILE != 0 {
        core::ptr::copy_nonoverlapping(TLS_IMAGE, data as *mut u8, TLS_FILE);
    }
    *(tp as *mut usize) = p as usize;
    (*p).tp = tp;
    p
}
unsafe fn free_thread(p: *mut Thread) {
    let stack = (*p).mapping;
    let size = (*p).mapping_len;
    let tls = (*p).tls;
    let tls_len = (*p).tls_len;
    let mut values = (*p).extra_values;
    while !values.is_null() {
        let next = (*values).next;
        unmap(values as *mut u8, PAGE);
        values = next;
    }
    let mut reads = (*p).reads;
    while !reads.is_null() {
        let next = (*reads).next;
        unmap(reads as *mut u8, PAGE);
        reads = next;
    }
    unmap(stack, size);
    unmap(tls, tls_len);
    unmap(
        p as *mut u8,
        rounded(core::mem::size_of::<Thread>(), PAGE).unwrap(),
    );
}
unsafe fn set_tp(tp: usize) {
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!("msr tpidr_el0, {}", in(reg) tp, options(nostack));
    #[cfg(target_arch = "x86_64")]
    {
        raw::syscall2(nr::ARCH_PRCTL, 0x1002, tp as u64);
    }
}

/// Read the executable's PT_TLS template from the kernel-supplied ELF headers.
/// No compiler TLS is accessed before this runs from libc's process entry.
pub unsafe fn startup(envp: *const *const u8) {
    let mut e = envp;
    while !(*e).is_null() {
        e = e.add(1);
    }
    let mut aux = e.add(1) as *const usize;
    let (mut phdr, mut count, mut stride) = (0usize, 0usize, 0usize);
    while *aux != 0 {
        match *aux {
            3 => phdr = *aux.add(1),
            4 => stride = *aux.add(1),
            5 => count = *aux.add(1),
            _ => {}
        }
        aux = aux.add(2);
    }
    if phdr != 0 && stride >= 56 {
        for i in 0..count {
            let h = (phdr + i * stride) as *const u8;
            if core::ptr::read_unaligned(h as *const u32) == 7 {
                TLS_IMAGE = core::ptr::read_unaligned(h.add(16) as *const usize) as *const u8;
                TLS_FILE = core::ptr::read_unaligned(h.add(32) as *const usize);
                TLS_SIZE = core::ptr::read_unaligned(h.add(40) as *const usize);
                TLS_ALIGN = core::ptr::read_unaligned(h.add(48) as *const usize).max(1);
            }
        }
    }
    let t = current();
    if t.is_null() {
        libbreenix::process::exit(127);
    }
    set_tp((*t).tp);
    RUNTIME_READY.store(true, Release);
}

unsafe fn registry_lock() {
    let process = pid();
    if PROCESS.load(Acquire) != process {
        // After fork only the calling thread survives. No inherited futex lock
        // can have an owner in the child; retain that thread's TLS and TSD.
        let id = tid();
        let mut p = HEAD;
        #[cfg(target_arch = "aarch64")]
        let inherited: usize = {
            let v;
            core::arch::asm!("mrs {}, tpidr_el0", out(reg) v, options(nomem, nostack));
            v
        };
        #[cfg(target_arch = "x86_64")]
        let inherited: usize = {
            let mut v = 0usize;
            raw::syscall2(nr::ARCH_PRCTL, 0x1003, &mut v as *mut usize as u64);
            v
        };
        let mut survivor = null_mut();
        while !p.is_null() {
            if (*p).tp == inherited {
                survivor = p;
                break;
            }
            p = (*p).next;
        }
        HEAD = survivor;
        LIVE.store(1, Relaxed);
        KEY_LOCK.0.store(0, Relaxed);
        REAPER_STARTED = false;
        if !survivor.is_null() {
            (*survivor).next = null_mut();
            (*survivor).id = id;
            (*survivor).clear.store(id, Relaxed);
            (*survivor).state = 0;
        }
        REGISTRY.0.store(0, Relaxed);
        PROCESS.store(process, Release);
    }
    REGISTRY.lock();
}
unsafe fn reap() {
    let mut link = core::ptr::addr_of_mut!(HEAD);
    while !(*link).is_null() {
        let p = *link;
        if (*p).state == 1 && (*p).clear.load(Acquire) == 0 {
            *link = (*p).next;
            free_thread(p);
        } else {
            link = core::ptr::addr_of_mut!((*p).next);
        }
    }
}
unsafe fn current() -> *mut Thread {
    let id = tid();
    if RUNTIME_READY.load(Acquire) && PROCESS.load(Relaxed) == pid() {
        let p = descriptor_from_tp();
        if !p.is_null() && (*p).id == id {
            return p;
        }
    }
    registry_lock();
    let mut p = HEAD;
    while !p.is_null() {
        if (*p).id == id {
            REGISTRY.unlock();
            return p;
        }
        p = (*p).next;
    }
    p = alloc_thread();
    if !p.is_null() {
        (*p).id = id;
        (*p).clear.store(id, Relaxed);
        (*p).next = HEAD;
        HEAD = p;
        LIVE.store(1, Release);
    }
    REGISTRY.unlock();
    p
}
/// Both ABI TCBs hold our descriptor in their first reserved word. Reading
/// errno does not take a lock or allocate, including from a signal handler.
unsafe fn descriptor_from_tp() -> *mut Thread {
    #[cfg(target_arch = "aarch64")]
    {
        let tp: usize;
        core::arch::asm!("mrs {}, tpidr_el0", out(reg) tp, options(nomem, nostack));
        if tp == 0 {
            null_mut()
        } else {
            *(tp as *const *mut Thread)
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        let p: *mut Thread;
        core::arch::asm!("mov {}, fs:[0]", out(reg) p, options(readonly, nostack));
        p
    }
}

pub fn errno_location() -> *mut i32 {
    unsafe {
        let p = if RUNTIME_READY.load(Acquire) {
            descriptor_from_tp()
        } else {
            current()
        };
        // Failure to allocate even the thread's C runtime state is fatal rather
        // than silently sharing errno with another thread.
        if p.is_null() {
            libbreenix::process::exit(127);
        }
        core::ptr::addr_of_mut!((*p).errno)
    }
}
unsafe fn find(handle: usize) -> *mut Thread {
    let mut p = HEAD;
    while !p.is_null() {
        if p as usize == handle {
            return p;
        }
        p = (*p).next;
    }
    null_mut()
}
#[no_mangle]
pub extern "C" fn pthread_self() -> usize {
    unsafe { current() as usize }
}
#[no_mangle]
pub extern "C" fn pthread_equal(a: usize, b: usize) -> i32 {
    (a == b) as i32
}

extern "C" fn entry(arg: u64) -> ! {
    unsafe {
        let p = arg as *mut Thread;
        set_tp((*p).tp);
        while (*p).ready.load(Acquire) == 0 {
            wait(&(*p).ready, 0);
        }
        if (*p).ready.load(Acquire) == 2 {
            libbreenix::process::exit(0);
        }
        let result = ((*p).start.unwrap())((*p).arg);
        pthread_exit(result)
    }
}
unsafe fn start_reaper() -> i32 {
    if REAPER_STARTED {
        return 0;
    }
    let p = alloc_thread();
    if p.is_null() {
        return EAGAIN;
    }
    let stack = map(32 * 1024, 3);
    if stack.is_null() {
        free_thread(p);
        return EAGAIN;
    }
    (*p).mapping = stack;
    (*p).mapping_len = 32 * 1024;
    let r = raw::syscall6(
        nr::CLONE,
        0x100 | 0x200 | 0x400 | 0x800 | 0x10000 | 0x80000 | 0x200000 | 0x1000000,
        stack.add(32 * 1024) as u64,
        reaper_entry as *const () as u64,
        p as u64,
        &(*p).clear as *const AtomicU32 as u64,
        (*p).tp as u64,
    ) as i64;
    if r < 0 {
        free_thread(p);
        return -r as i32;
    }
    REAPER_STARTED = true;
    0
}
extern "C" fn reaper_entry(arg: u64) -> ! {
    unsafe {
        let own = arg as *mut Thread;
        set_tp((*own).tp);
        loop {
            registry_lock();
            let observed = REAPER_EVENT.load(Acquire);
            let mut p = HEAD;
            while !p.is_null() && !((*p).state == 1 && (*p).exiting) {
                p = (*p).next;
            }
            if p.is_null() {
                REGISTRY.unlock();
                wait(&REAPER_EVENT, observed);
                continue;
            }
            (*p).state = 3;
            REGISTRY.unlock();
            loop {
                let id = (*p).clear.load(Acquire);
                if id == 0 {
                    break;
                }
                wait(&(*p).clear, id);
            }
            registry_lock();
            let mut link = core::ptr::addr_of_mut!(HEAD);
            while *link != p {
                link = core::ptr::addr_of_mut!((**link).next);
            }
            *link = (*p).next;
            free_thread(p);
            REGISTRY.unlock();
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_create(
    out: *mut usize,
    attr: *const u8,
    start: Start,
    arg: *mut u8,
) -> i32 {
    if out.is_null() {
        return EINVAL;
    }
    if current().is_null() {
        return ENOMEM;
    }
    registry_lock();
    reap();
    let a = if attr.is_null() {
        Attr::new()
    } else {
        *(attr as *const Attr)
    };
    // Inherit the calling thread's actual policy, not the attribute defaults.
    // An older kernel returning ENOSYS still creates ordinary inherited threads.
    let mut policy = a.policy;
    let mut priority = a.priority;
    if a.inherit == 0 {
        let inherited = raw::syscall1(nr::SCHED_GETSCHEDULER, 0) as i64;
        policy = if inherited >= 0 { inherited as i32 } else { 0 };
        priority = 0;
        if inherited >= 0 {
            let r = raw::syscall2(nr::SCHED_GETPARAM, 0, &mut priority as *mut i32 as u64) as i64;
            if r < 0 {
                REGISTRY.unlock();
                return -r as i32;
            }
        }
    }
    if a.detached == 1 {
        let r = start_reaper();
        if r != 0 {
            REGISTRY.unlock();
            return r;
        }
    }
    let p = alloc_thread();
    if p.is_null() {
        REGISTRY.unlock();
        return EAGAIN;
    }
    (*p).attr = a;
    (*p).start = Some(start);
    (*p).arg = arg;
    let stack;
    if a.stack.is_null() {
        let Some(guard) = rounded(a.guard, PAGE) else {
            free_thread(p);
            REGISTRY.unlock();
            return EINVAL;
        };
        let Some(len) = a.size.checked_add(guard).and_then(|v| rounded(v, PAGE)) else {
            free_thread(p);
            REGISTRY.unlock();
            return EINVAL;
        };
        let base = map(len, 0);
        if base.is_null() {
            free_thread(p);
            REGISTRY.unlock();
            return EAGAIN;
        }
        (*p).mapping = base;
        (*p).mapping_len = len;
        stack = base.add(guard);
        let r = raw::syscall3(nr::MPROTECT, stack as u64, (len - guard) as u64, 3) as i64;
        if r < 0 {
            free_thread(p);
            REGISTRY.unlock();
            return -r as i32;
        }
        (*p).attr.stack = stack;
    } else {
        stack = a.stack;
    }
    let Some(top) = (stack as usize).checked_add(a.size) else {
        free_thread(p);
        REGISTRY.unlock();
        return EINVAL;
    };
    (*p).clear.store(u32::MAX, Relaxed);
    // Breenix's clone ABI supplies entry and argument explicitly. The sixth
    // argument is the TLS pointer for the kernel lane's CLONE_SETTLS extension.
    // Until that extension lands, entry installs the pointer before user code.
    let flags = 0x100 | 0x200 | 0x400 | 0x800 | 0x10000 | 0x80000 | 0x200000 | 0x1000000;
    let r = raw::syscall6(
        nr::CLONE,
        flags,
        (top & !15) as u64,
        entry as *const () as u64,
        p as u64,
        &(*p).clear as *const AtomicU32 as u64,
        (*p).tp as u64,
    ) as i64;
    if r < 0 {
        free_thread(p);
        REGISTRY.unlock();
        return -r as i32;
    }
    (*p).id = r as u32;
    (*p).next = HEAD;
    HEAD = p;
    if a.inherit == 1 || policy != 0 {
        let result = raw::syscall3(
            nr::SCHED_SETSCHEDULER,
            r as u64,
            policy as u64,
            &priority as *const i32 as u64,
        ) as i64;
        if result < 0 {
            // A failed creation must not leave a descriptor for the caller to
            // collect. The child never ran the application routine.
            (*p).state = 2;
            (*p).ready.store(2, Release);
            wake(&(*p).ready, 1);
            REGISTRY.unlock();
            loop {
                let id = (*p).clear.load(Acquire);
                if id == 0 {
                    break;
                }
                wait(&(*p).clear, id);
            }
            registry_lock();
            let mut link = core::ptr::addr_of_mut!(HEAD);
            while *link != p {
                link = core::ptr::addr_of_mut!((**link).next);
            }
            *link = (*p).next;
            free_thread(p);
            REGISTRY.unlock();
            return -result as i32;
        }
    }
    *out = p as usize;
    (*p).state = a.detached as u32;
    LIVE.fetch_add(1, Release);
    (*p).ready.store(1, Release);
    wake(&(*p).ready, 1);
    REGISTRY.unlock();
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_join(handle: usize, value: *mut *mut u8) -> i32 {
    if handle == pthread_self() {
        return EDEADLK;
    }
    registry_lock();
    let p = find(handle);
    if p.is_null() {
        REGISTRY.unlock();
        return ESRCH;
    }
    if (*p).state != 0 {
        REGISTRY.unlock();
        return EINVAL;
    }
    (*p).state = 2;
    REGISTRY.unlock();
    loop {
        let id = (*p).clear.load(Acquire);
        if id == 0 {
            break;
        }
        wait(&(*p).clear, id);
    }
    if !value.is_null() {
        *value = (*p).result;
    }
    registry_lock();
    let mut link = core::ptr::addr_of_mut!(HEAD);
    while *link != p {
        link = core::ptr::addr_of_mut!((**link).next);
    }
    *link = (*p).next;
    free_thread(p);
    REGISTRY.unlock();
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_detach(handle: usize) -> i32 {
    registry_lock();
    let p = find(handle);
    if p.is_null() {
        REGISTRY.unlock();
        return ESRCH;
    }
    if (*p).state != 0 {
        REGISTRY.unlock();
        return EINVAL;
    }
    let r = start_reaper();
    if r != 0 {
        REGISTRY.unlock();
        return r;
    }
    (*p).state = 1;
    REAPER_EVENT.fetch_add(1, Release);
    wake(&REAPER_EVENT, 1);
    REGISTRY.unlock();
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_exit(value: *mut u8) -> ! {
    let p = current();
    if !p.is_null() {
        destructors(p);
        while !(*p).robust.is_null() {
            let m = (*p).robust;
            (*p).robust = (*m).next;
            (*m).recovery.store(1, Relaxed);
            (*m).depth = 0;
            (*m).owner.store(0, Release);
            wake(&(*m).owner, u32::MAX);
        }
        (*p).result = value;
        registry_lock();
        (*p).exiting = true;
        REAPER_EVENT.fetch_add(1, Release);
        wake(&REAPER_EVENT, 1);
        REGISTRY.unlock();
        // The internal reaper does not keep the application alive after its
        // last user thread exits. Process termination also releases its stack.
        if LIVE.fetch_sub(1, Release) == 1 {
            super::exit_group(0);
        }
    }
    libbreenix::process::exit(0)
}

#[no_mangle]
pub unsafe extern "C" fn pthread_getattr_np(handle: usize, attr: *mut u8) -> i32 {
    if attr.is_null() {
        return EINVAL;
    }
    registry_lock();
    let p = find(handle);
    if p.is_null() {
        REGISTRY.unlock();
        return ESRCH;
    }
    *(attr as *mut Attr) = (*p).attr;
    REGISTRY.unlock();
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_attr_init(a: *mut u8) -> i32 {
    if a.is_null() {
        return EINVAL;
    }
    *(a as *mut Attr) = Attr::new();
    0
}
#[no_mangle]
pub extern "C" fn pthread_attr_destroy(a: *mut u8) -> i32 {
    if a.is_null() {
        EINVAL
    } else {
        0
    }
}
macro_rules! attr_field {
    ($set:ident, $get:ident, $field:ident, $ty:ty, $valid:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $set(a: *mut u8, v: $ty) -> i32 {
            if a.is_null() || !($valid)(v) {
                return EINVAL;
            }
            (*(a as *mut Attr)).$field = v;
            0
        }
        #[no_mangle]
        pub unsafe extern "C" fn $get(a: *const u8, v: *mut $ty) -> i32 {
            if a.is_null() || v.is_null() {
                return EINVAL;
            }
            *v = (*(a as *const Attr)).$field;
            0
        }
    };
}
attr_field!(
    pthread_attr_setdetachstate,
    pthread_attr_getdetachstate,
    detached,
    i32,
    |v| v == 0 || v == 1
);
attr_field!(
    pthread_attr_setstacksize,
    pthread_attr_getstacksize,
    size,
    usize,
    |v| v >= STACK_MIN && v <= isize::MAX as usize
);
attr_field!(
    pthread_attr_setguardsize,
    pthread_attr_getguardsize,
    guard,
    usize,
    |v| v <= isize::MAX as usize
);
attr_field!(
    pthread_attr_setinheritsched,
    pthread_attr_getinheritsched,
    inherit,
    i32,
    |v| v == 0 || v == 1
);
attr_field!(
    pthread_attr_setschedpolicy,
    pthread_attr_getschedpolicy,
    policy,
    i32,
    |v| (0..=2).contains(&v)
);
#[no_mangle]
pub unsafe extern "C" fn pthread_attr_setschedparam(a: *mut u8, p: *const i32) -> i32 {
    if a.is_null() || p.is_null() || !(0..=99).contains(&*p) {
        return EINVAL;
    }
    (*(a as *mut Attr)).priority = *p;
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_attr_getschedparam(a: *const u8, p: *mut i32) -> i32 {
    if a.is_null() || p.is_null() {
        return EINVAL;
    }
    *p = (*(a as *const Attr)).priority;
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_attr_setstack(a: *mut u8, addr: *mut u8, size: usize) -> i32 {
    if a.is_null()
        || addr.is_null()
        || size < STACK_MIN
        || (addr as usize).checked_add(size).is_none()
    {
        return EINVAL;
    }
    let a = &mut *(a as *mut Attr);
    a.stack = addr;
    a.size = size;
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_attr_getstack(
    a: *const u8,
    addr: *mut *mut u8,
    size: *mut usize,
) -> i32 {
    if a.is_null() || addr.is_null() || size.is_null() {
        return EINVAL;
    }
    *addr = (*(a as *const Attr)).stack;
    *size = (*(a as *const Attr)).size;
    0
}
#[no_mangle]
pub extern "C" fn pthread_setname_np(thread: usize, name: *const u8) -> i32 {
    if thread == 0 || name.is_null() {
        EINVAL
    } else {
        0
    }
}

// Keys and values grow in stable mmap pages. No hard key limit is advertised:
// key creation can exhaust address-space resources, but runtime-owned keys do
// not subtract from the POSIX minimum available to application code.
struct Key {
    generation: u32,
    destructor: Destructor,
}
#[repr(C)]
struct KeyBlock {
    next: *mut KeyBlock,
    keys: [Key; KEYS],
}
#[repr(C)]
struct Values {
    next: *mut Values,
    block: u32,
    entries: [(*mut u8, u32); KEYS],
}
static KEY_LOCK: Lock = Lock::new();
static mut KEY_BLOCKS: *mut KeyBlock = null_mut();
unsafe fn key_at(k: u32) -> *mut Key {
    let mut p = KEY_BLOCKS;
    for _ in 0..k as usize / KEYS {
        if p.is_null() {
            return null_mut();
        }
        p = (*p).next;
    }
    if p.is_null() {
        null_mut()
    } else {
        core::ptr::addr_of_mut!((*p).keys[k as usize % KEYS])
    }
}
unsafe fn value_at(p: *mut Thread, k: u32, allocate: bool) -> *mut (*mut u8, u32) {
    let block = k / KEYS as u32;
    if block == 0 {
        return core::ptr::addr_of_mut!((*p).values[k as usize]);
    }
    let mut link = core::ptr::addr_of_mut!((*p).extra_values);
    while !(*link).is_null() {
        if (**link).block == block {
            return core::ptr::addr_of_mut!((**link).entries[k as usize % KEYS]);
        }
        link = core::ptr::addr_of_mut!((**link).next);
    }
    if !allocate {
        return null_mut();
    }
    let page = map(PAGE, 3) as *mut Values;
    if page.is_null() {
        return null_mut();
    }
    (*page).block = block;
    *link = page;
    core::ptr::addr_of_mut!((*page).entries[k as usize % KEYS])
}
#[no_mangle]
pub unsafe extern "C" fn pthread_key_create(out: *mut u32, destructor: Destructor) -> i32 {
    if out.is_null() {
        return EINVAL;
    }
    KEY_LOCK.lock();
    let mut link = core::ptr::addr_of_mut!(KEY_BLOCKS);
    let mut base = 0u32;
    loop {
        if (*link).is_null() {
            *link = map(PAGE, 3) as *mut KeyBlock;
            if (*link).is_null() {
                KEY_LOCK.unlock();
                return EAGAIN;
            }
        }
        for (i, key) in (**link).keys.iter_mut().enumerate() {
            if key.generation & 1 == 0 {
                key.destructor = destructor;
                key.generation = key.generation.wrapping_add(1);
                *out = base + i as u32;
                KEY_LOCK.unlock();
                return 0;
            }
        }
        let Some(next_base) = base.checked_add(KEYS as u32) else {
            KEY_LOCK.unlock();
            return EAGAIN;
        };
        base = next_base;
        link = core::ptr::addr_of_mut!((**link).next);
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_key_delete(k: u32) -> i32 {
    KEY_LOCK.lock();
    let key = key_at(k);
    if key.is_null() || (*key).generation & 1 == 0 {
        KEY_LOCK.unlock();
        return EINVAL;
    }
    (*key).generation = (*key).generation.wrapping_add(1);
    (*key).destructor = None;
    KEY_LOCK.unlock();
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_setspecific(k: u32, value: *mut u8) -> i32 {
    let p = current();
    if p.is_null() {
        return ENOMEM;
    }
    KEY_LOCK.lock();
    let key = key_at(k);
    if key.is_null() || (*key).generation & 1 == 0 {
        KEY_LOCK.unlock();
        return EINVAL;
    }
    let entry = value_at(p, k, true);
    if entry.is_null() {
        KEY_LOCK.unlock();
        return ENOMEM;
    }
    *entry = (value, (*key).generation);
    KEY_LOCK.unlock();
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_getspecific(k: u32) -> *mut u8 {
    let p = current();
    if p.is_null() {
        return null_mut();
    }
    KEY_LOCK.lock();
    let key = key_at(k);
    let entry = value_at(p, k, false);
    let value = if !key.is_null()
        && !entry.is_null()
        && (*key).generation & 1 != 0
        && (*entry).1 == (*key).generation
    {
        (*entry).0
    } else {
        null_mut()
    };
    KEY_LOCK.unlock();
    value
}
unsafe fn destructors(p: *mut Thread) {
    for _ in 0..4 {
        let mut called = false;
        let mut key = 0u32;
        loop {
            KEY_LOCK.lock();
            let definition = key_at(key);
            if definition.is_null() {
                KEY_LOCK.unlock();
                break;
            }
            let entry = value_at(p, key, false);
            let (value, destructor) = if !entry.is_null() {
                let (value, g) = *entry;
                (*entry).0 = null_mut();
                (
                    value,
                    if g & 1 != 0 && g == (*definition).generation {
                        (*definition).destructor
                    } else {
                        None
                    },
                )
            } else {
                (null_mut(), None)
            };
            KEY_LOCK.unlock();
            if !value.is_null() {
                if let Some(f) = destructor {
                    called = true;
                    f(value);
                }
            }
            key += 1;
        }
        if !called {
            break;
        }
    }
}

fn valid_time(t: &[i64; 2]) -> bool {
    (0..1_000_000_000).contains(&t[1])
}
unsafe fn deadline_wait(p: &AtomicU32, value: u32, deadline: *const i64, clock: i32) -> i32 {
    if deadline.is_null() {
        wait(p, value);
        return 0;
    }
    let t = core::ptr::read_unaligned(deadline as *const [i64; 2]);
    if !valid_time(&t) {
        return EINVAL;
    }
    let op = 9 | if clock == 0 { 256 } else { 0 };
    let r = futex(p, op, value, t.as_ptr());
    if r != -38 {
        return if r == -ETIMEDOUT {
            ETIMEDOUT
        } else if r == -EINVAL {
            EINVAL
        } else {
            0
        };
    }
    // Main currently has relative FUTEX_WAIT only. Bound each sleep so that
    // CLOCK_REALTIME adjustments are noticed; prefer the exact absolute kernel
    // operation as soon as the other lane implements it.
    let mut now = [0i64; 2];
    let r = raw::syscall2(nr::CLOCK_GETTIME, clock as u64, now.as_mut_ptr() as u64) as i64;
    if r < 0 {
        return -r as i32;
    }
    let ns = (t[0] as i128 - now[0] as i128) * 1_000_000_000 + t[1] as i128 - now[1] as i128;
    if ns <= 0 {
        return ETIMEDOUT;
    }
    let ns = ns.min(if clock == 0 {
        10_000_000
    } else {
        i64::MAX as i128
    });
    let relative = [(ns / 1_000_000_000) as i64, (ns % 1_000_000_000) as i64];
    futex(p, 0, value, relative.as_ptr());
    0
}

// A mutex's owner is also its futex word. Unlock always wakes a waiter, avoiding
// a separate waiters bit and its publication races. The small extra syscall on
// an uncontended unlock buys a single source of truth for errorcheck/recursive.
#[repr(C)]
struct Mutex {
    owner: AtomicU32,
    kind: u32,
    depth: u32,
    recovery: AtomicU32, // 0 healthy, 1 owner dead, 2 not recoverable
    next: *mut Mutex,    // private to the owner, used only for robust mutexes
}
const ROBUST: u32 = 4;
unsafe fn mutex_take(m: *mut Mutex, attempt: bool, deadline: *const i64) -> i32 {
    if m.is_null() {
        return EINVAL;
    }
    let id = tid();
    loop {
        if (*m).recovery.load(Acquire) == 2 {
            return ENOTRECOVERABLE;
        }
        match (*m).owner.compare_exchange(0, id, Acquire, Relaxed) {
            Ok(_) => {
                // A recovering owner can mark the mutex unrecoverable between
                // our initial check and acquisition. Never admit that race.
                if (*m).recovery.load(Acquire) == 2 {
                    (*m).owner.store(0, Release);
                    wake(&(*m).owner, u32::MAX);
                    return ENOTRECOVERABLE;
                }
                (*m).depth = 1;
                if (*m).kind & ROBUST != 0 {
                    let t = current();
                    (*m).next = (*t).robust;
                    (*t).robust = m;
                }
                return if (*m).recovery.load(Relaxed) == 1 {
                    EOWNERDEAD
                } else {
                    0
                };
            }
            Err(owner) => {
                if owner == id {
                    if (*m).kind & 3 == 1 {
                        let Some(depth) = (*m).depth.checked_add(1) else {
                            return EAGAIN;
                        };
                        (*m).depth = depth;
                        return 0;
                    }
                    if !attempt && (*m).kind & 3 == 2 {
                        return EDEADLK;
                    }
                }
                if attempt {
                    return EBUSY;
                }
                let r = deadline_wait(&(*m).owner, owner, deadline, 0);
                if r != 0 {
                    return r;
                }
            }
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_init(m: *mut u8, a: *const u8) -> i32 {
    if m.is_null() {
        return EINVAL;
    }
    core::ptr::write(
        m as *mut Mutex,
        Mutex {
            owner: AtomicU32::new(0),
            kind: if a.is_null() { 0 } else { *(a as *const u32) },
            depth: 0,
            recovery: AtomicU32::new(0),
            next: null_mut(),
        },
    );
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_destroy(m: *mut u8) -> i32 {
    if m.is_null() {
        return EINVAL;
    }
    if (*(m as *mut Mutex)).owner.load(Acquire) != 0 {
        EBUSY
    } else {
        0
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_lock(m: *mut u8) -> i32 {
    mutex_take(m as *mut Mutex, false, null())
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_trylock(m: *mut u8) -> i32 {
    mutex_take(m as *mut Mutex, true, null())
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_timedlock(m: *mut u8, t: *const i64) -> i32 {
    if t.is_null() {
        EINVAL
    } else {
        mutex_take(m as *mut Mutex, false, t)
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_unlock(m: *mut u8) -> i32 {
    if m.is_null() {
        return EINVAL;
    }
    let m = m as *mut Mutex;
    if (*m).kind != 0 && (*m).owner.load(Relaxed) != tid() {
        return EPERM;
    }
    if (*m).kind & 3 == 1 && (*m).depth > 1 {
        (*m).depth -= 1;
        return 0;
    }
    if (*m).kind & ROBUST != 0 {
        let t = current();
        let mut link = core::ptr::addr_of_mut!((*t).robust);
        while !(*link).is_null() && *link != m {
            link = core::ptr::addr_of_mut!((**link).next);
        }
        if *link == m {
            *link = (*m).next;
        }
        if (*m).recovery.load(Relaxed) == 1 {
            (*m).recovery.store(2, Relaxed);
        }
    }
    (*m).depth = 0;
    (*m).owner.store(0, Release);
    let count = if (*m).recovery.load(Relaxed) == 2 {
        u32::MAX
    } else {
        1
    };
    wake(&(*m).owner, count);
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_consistent(m: *mut u8) -> i32 {
    if m.is_null() {
        return EINVAL;
    }
    let m = &*(m as *mut Mutex);
    if m.kind & ROBUST == 0
        || m.owner.load(Relaxed) != tid()
        || m.recovery.compare_exchange(1, 0, Relaxed, Relaxed).is_err()
    {
        EINVAL
    } else {
        0
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_init(a: *mut u8) -> i32 {
    if a.is_null() {
        return EINVAL;
    }
    *(a as *mut u32) = 0;
    0
}
#[no_mangle]
pub extern "C" fn pthread_mutexattr_destroy(a: *mut u8) -> i32 {
    if a.is_null() {
        EINVAL
    } else {
        0
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_settype(a: *mut u8, v: i32) -> i32 {
    if a.is_null() || !(0..=2).contains(&v) {
        return EINVAL;
    }
    *(a as *mut u32) = (*(a as *mut u32) & !3) | v as u32;
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_gettype(a: *const u8, v: *mut i32) -> i32 {
    if a.is_null() || v.is_null() {
        return EINVAL;
    }
    *v = (*(a as *const u32) & 3) as i32;
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_setrobust(a: *mut u8, v: i32) -> i32 {
    if a.is_null() || !(0..=1).contains(&v) {
        return EINVAL;
    }
    *(a as *mut u32) = (*(a as *mut u32) & !ROBUST) | (v as u32 * ROBUST);
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_getrobust(a: *const u8, v: *mut i32) -> i32 {
    if a.is_null() || v.is_null() {
        return EINVAL;
    }
    *v = ((*(a as *const u32) & ROBUST) != 0) as i32;
    0
}
#[no_mangle]
pub extern "C" fn pthread_mutexattr_setprotocol(a: *mut u8, v: i32) -> i32 {
    if a.is_null() || !(0..=2).contains(&v) {
        EINVAL
    } else if v != 0 {
        95
    } else {
        0
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_getprotocol(a: *const u8, v: *mut i32) -> i32 {
    if a.is_null() || v.is_null() {
        return EINVAL;
    }
    *v = 0;
    0
}

#[repr(C)]
struct Cond {
    sequence: AtomicU32,
    clock: i32,
}
#[no_mangle]
pub unsafe extern "C" fn pthread_cond_init(c: *mut u8, a: *const u8) -> i32 {
    if c.is_null() {
        return EINVAL;
    }
    core::ptr::write(
        c as *mut Cond,
        Cond {
            sequence: AtomicU32::new(0),
            clock: if a.is_null() { 0 } else { *(a as *const i32) },
        },
    );
    0
}
#[no_mangle]
pub extern "C" fn pthread_cond_destroy(c: *mut u8) -> i32 {
    if c.is_null() {
        EINVAL
    } else {
        0
    }
}
unsafe fn cond_wake(c: *mut u8, count: u32) -> i32 {
    if c.is_null() {
        return EINVAL;
    }
    let c = &*(c as *const Cond);
    c.sequence.fetch_add(1, Release);
    wake(&c.sequence, count);
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_cond_signal(c: *mut u8) -> i32 {
    cond_wake(c, 1)
}
#[no_mangle]
pub unsafe extern "C" fn pthread_cond_broadcast(c: *mut u8) -> i32 {
    cond_wake(c, u32::MAX)
}
unsafe fn cond_wait(c: *mut u8, m: *mut u8, t: *const i64) -> i32 {
    if c.is_null() || m.is_null() {
        return EINVAL;
    }
    if !t.is_null() && !valid_time(&core::ptr::read_unaligned(t as *const [i64; 2])) {
        return EINVAL;
    }
    let c = &*(c as *const Cond);
    let seq = c.sequence.load(Acquire);
    let r = pthread_mutex_unlock(m);
    if r != 0 {
        return r;
    }
    let mut r;
    loop {
        r = deadline_wait(&c.sequence, seq, t, c.clock);
        if r != 0 || c.sequence.load(Acquire) != seq {
            break;
        }
    }
    let lock = pthread_mutex_lock(m);
    if lock != 0 {
        lock
    } else {
        r
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_cond_wait(c: *mut u8, m: *mut u8) -> i32 {
    cond_wait(c, m, null())
}
#[no_mangle]
pub unsafe extern "C" fn pthread_cond_timedwait(c: *mut u8, m: *mut u8, t: *const i64) -> i32 {
    if t.is_null() {
        EINVAL
    } else {
        cond_wait(c, m, t)
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_condattr_init(a: *mut u8) -> i32 {
    if a.is_null() {
        return EINVAL;
    }
    *(a as *mut i32) = 0;
    0
}
#[no_mangle]
pub extern "C" fn pthread_condattr_destroy(a: *mut u8) -> i32 {
    if a.is_null() {
        EINVAL
    } else {
        0
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_condattr_setclock(a: *mut u8, clock: i32) -> i32 {
    if a.is_null() || !(0..=1).contains(&clock) {
        return EINVAL;
    }
    *(a as *mut i32) = clock;
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_condattr_getclock(a: *const u8, clock: *mut i32) -> i32 {
    if a.is_null() || clock.is_null() {
        return EINVAL;
    }
    *clock = *(a as *const i32);
    0
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ReadHold {
    lock: *mut Rwlock,
    count: u32,
}
#[repr(C)]
struct ReadBlock {
    next: *mut ReadBlock,
    entries: [ReadHold; 128],
}
unsafe fn read_hold(p: *mut Thread, rw: *mut Rwlock, allocate: bool) -> *mut ReadHold {
    let mut link = core::ptr::addr_of_mut!((*p).reads);
    let mut empty: *mut ReadHold = null_mut();
    while !(*link).is_null() {
        for entry in (**link).entries.iter_mut() {
            if entry.lock == rw && entry.count != 0 {
                return entry;
            }
            if entry.count == 0 && empty.is_null() {
                empty = entry as *mut ReadHold;
            }
        }
        link = core::ptr::addr_of_mut!((**link).next);
    }
    if !allocate {
        return null_mut();
    }
    if empty.is_null() {
        let block = map(PAGE, 3) as *mut ReadBlock;
        if block.is_null() {
            return null_mut();
        }
        *link = block;
        empty = core::ptr::addr_of_mut!((*block).entries[0]);
    }
    (*empty).lock = rw;
    empty
}
#[repr(C)]
struct Rwlock {
    gate: Lock,
    readers: u32,
    writer: u32,
    writers_waiting: u32,
    writers: *mut RwWaiter,
    sequence: AtomicU32,
}
struct RwWaiter {
    next: *mut RwWaiter,
    policy: i32,
    priority: i32,
}
unsafe fn scheduling() -> (i32, i32) {
    let policy = raw::syscall1(nr::SCHED_GETSCHEDULER, 0) as i64;
    let mut priority = 0;
    if policy == 1 || policy == 2 {
        raw::syscall2(nr::SCHED_GETPARAM, 0, &mut priority as *mut i32 as u64);
    }
    (if policy >= 0 { policy as i32 } else { 0 }, priority)
}
unsafe fn remove_writer(r: *mut Rwlock, w: *mut RwWaiter) {
    let mut link = core::ptr::addr_of_mut!((*r).writers);
    while *link != w {
        link = core::ptr::addr_of_mut!((**link).next);
    }
    *link = (*w).next;
    (*r).writers_waiting -= 1;
}
unsafe fn reader_admitted(r: *mut Rwlock, recursive: bool) -> bool {
    if (*r).writer != 0 {
        return false;
    }
    if recursive || (*r).writers_waiting == 0 {
        return true;
    }
    let (policy, priority) = scheduling();
    // POSIX permits writer preference for SCHED_OTHER. With FIFO/RR, only
    // pending realtime writers of at least the reader's priority exclude it.
    if policy != 1 && policy != 2 {
        return false;
    }
    let mut w = (*r).writers;
    while !w.is_null() {
        if ((*w).policy == 1 || (*w).policy == 2) && (*w).priority >= priority {
            return false;
        }
        w = (*w).next;
    }
    true
}

// Waiting writers block new readers; an existing reader may recurse even
// while a writer is queued, so recursive reads cannot deadlock behind themselves.
unsafe fn rw_take(p: *mut u8, write: bool, attempt: bool, deadline: *const i64) -> i32 {
    if p.is_null() {
        return EINVAL;
    }
    let r = p as *mut Rwlock;
    let thread = current();
    if thread.is_null() {
        return EAGAIN;
    }
    let held = if write {
        null_mut()
    } else {
        read_hold(thread, r, true)
    };
    if !write && held.is_null() {
        return EAGAIN;
    }
    let (policy, priority) = if write && !attempt {
        scheduling()
    } else {
        (0, 0)
    };
    let mut waiter = RwWaiter {
        next: null_mut(),
        policy,
        priority,
    };
    if attempt {
        if !(*r).gate.try_lock() {
            return EBUSY;
        }
    } else {
        (*r).gate.lock();
    }
    if write && !attempt {
        waiter.next = (*r).writers;
        (*r).writers = &mut waiter;
        (*r).writers_waiting += 1;
    }
    loop {
        let available = if write {
            (*r).writer == 0 && (*r).readers == 0
        } else {
            reader_admitted(r, (*held).count != 0)
        };
        if available {
            if write {
                (*r).writer = tid();
                if !attempt {
                    remove_writer(r, &mut waiter);
                }
            } else {
                if (*r).readers == u32::MAX {
                    (*r).gate.unlock();
                    return EAGAIN;
                }
                (*r).readers += 1;
                (*held).count += 1;
            }
            (*r).gate.unlock();
            return 0;
        }
        if attempt {
            (*r).gate.unlock();
            return EBUSY;
        }
        let seq = (*r).sequence.load(Relaxed);
        (*r).gate.unlock();
        let result = deadline_wait(&(*r).sequence, seq, deadline, 0);
        (*r).gate.lock();
        if result != 0 {
            if write {
                remove_writer(r, &mut waiter);
                (*r).sequence.fetch_add(1, Release);
                wake(&(*r).sequence, u32::MAX);
            }
            (*r).gate.unlock();
            return result;
        }
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_init(p: *mut u8, attr: *const u8) -> i32 {
    if p.is_null() {
        return EINVAL;
    }
    if !attr.is_null() && *(attr as *const i32) != 0 {
        return 95;
    }
    core::ptr::write(
        p as *mut Rwlock,
        Rwlock {
            gate: Lock::new(),
            readers: 0,
            writer: 0,
            writers_waiting: 0,
            writers: null_mut(),
            sequence: AtomicU32::new(0),
        },
    );
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_destroy(p: *mut u8) -> i32 {
    if p.is_null() {
        return EINVAL;
    }
    let r = &*(p as *const Rwlock);
    r.gate.lock();
    let busy = r.readers != 0 || r.writer != 0 || r.writers_waiting != 0;
    r.gate.unlock();
    if busy {
        EBUSY
    } else {
        0
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_rdlock(p: *mut u8) -> i32 {
    rw_take(p, false, false, null())
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_wrlock(p: *mut u8) -> i32 {
    rw_take(p, true, false, null())
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_tryrdlock(p: *mut u8) -> i32 {
    rw_take(p, false, true, null())
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_trywrlock(p: *mut u8) -> i32 {
    rw_take(p, true, true, null())
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_timedrdlock(p: *mut u8, t: *const i64) -> i32 {
    if t.is_null() {
        EINVAL
    } else {
        rw_take(p, false, false, t)
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_timedwrlock(p: *mut u8, t: *const i64) -> i32 {
    if t.is_null() {
        EINVAL
    } else {
        rw_take(p, true, false, t)
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_unlock(p: *mut u8) -> i32 {
    if p.is_null() {
        return EINVAL;
    }
    let r = p as *mut Rwlock;
    (*r).gate.lock();
    if (*r).writer != 0 {
        if (*r).writer != tid() {
            (*r).gate.unlock();
            return EPERM;
        }
        (*r).writer = 0;
    } else if (*r).readers != 0 {
        let held = read_hold(current(), r, false);
        if held.is_null() {
            (*r).gate.unlock();
            return EPERM;
        }
        (*held).count -= 1;
        (*r).readers -= 1;
    } else {
        (*r).gate.unlock();
        return EPERM;
    }
    (*r).sequence.fetch_add(1, Release);
    (*r).gate.unlock();
    wake(&(*r).sequence, u32::MAX);
    0
}
#[repr(C)]
struct Barrier {
    gate: Lock,
    count: u32,
    arrived: u32,
    generation: AtomicU32,
}
#[no_mangle]
pub unsafe extern "C" fn pthread_barrier_init(p: *mut u8, attr: *const u8, count: u32) -> i32 {
    if p.is_null() || count == 0 {
        return EINVAL;
    }
    if !attr.is_null() && *(attr as *const i32) != 0 {
        return 95;
    }
    core::ptr::write(
        p as *mut Barrier,
        Barrier {
            gate: Lock::new(),
            count,
            arrived: 0,
            generation: AtomicU32::new(0),
        },
    );
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_barrier_wait(p: *mut u8) -> i32 {
    if p.is_null() {
        return EINVAL;
    }
    let b = p as *mut Barrier;
    (*b).gate.lock();
    let g = (*b).generation.load(Relaxed);
    (*b).arrived += 1;
    if (*b).arrived == (*b).count {
        (*b).arrived = 0;
        (*b).generation.fetch_add(1, Release);
        (*b).gate.unlock();
        wake(&(*b).generation, u32::MAX);
        return -1;
    }
    (*b).gate.unlock();
    while (*b).generation.load(Acquire) == g {
        wait(&(*b).generation, g);
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_barrier_destroy(p: *mut u8) -> i32 {
    if p.is_null() {
        return EINVAL;
    }
    let b = &*(p as *const Barrier);
    b.gate.lock();
    let busy = b.arrived != 0;
    b.gate.unlock();
    if busy {
        EBUSY
    } else {
        0
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_once(p: *mut i32, init: extern "C" fn()) -> i32 {
    if p.is_null() {
        return EINVAL;
    }
    let w = word(p as *const u32);
    if w.compare_exchange(0, 1, Acquire, Acquire).is_ok() {
        init();
        w.store(2, Release);
        wake(w, u32::MAX);
    } else {
        while w.load(Acquire) != 2 {
            wait(w, 1);
        }
    }
    0
}

pub fn signal_current(sig: i32) -> i64 {
    unsafe { raw::syscall3(nr::TGKILL, pid() as u64, tid() as u64, sig as u64) as i64 }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_kill(handle: usize, sig: i32) -> i32 {
    if !(0..=64).contains(&sig) {
        return EINVAL;
    }
    registry_lock();
    let p = find(handle);
    if p.is_null() {
        REGISTRY.unlock();
        return ESRCH;
    }
    let id = (*p).id;
    REGISTRY.unlock();
    let r = raw::syscall3(nr::TGKILL, pid() as u64, id as u64, sig as u64) as i64;
    if r < 0 {
        -r as i32
    } else {
        0
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_sigmask(how: i32, set: *const u64, old: *mut u64) -> i32 {
    let r = raw::syscall4(nr::SIGPROCMASK, how as u64, set as u64, old as u64, 8) as i64;
    if r < 0 {
        -r as i32
    } else {
        0
    }
}
#[no_mangle]
pub unsafe extern "C" fn sigwait(set: *const u64, sig: *mut i32) -> i32 {
    if set.is_null() || sig.is_null() {
        return EINVAL;
    }
    loop {
        let r = raw::syscall4(nr::SIGTIMEDWAIT, set as u64, 0, 0, 8) as i64;
        if r == -4 {
            continue;
        }
        if r < 0 {
            return -r as i32;
        }
        *sig = r as i32;
        return 0;
    }
}
#[no_mangle]
pub unsafe extern "C" fn sched_setscheduler(id: i32, policy: i32, param: *const i32) -> i32 {
    super::syscall_result_to_c_int(raw::syscall3(
        nr::SCHED_SETSCHEDULER,
        id as u64,
        policy as u64,
        param as u64,
    ) as i64)
}
#[no_mangle]
pub unsafe extern "C" fn sched_getscheduler(id: i32) -> i32 {
    super::syscall_result_to_c_int(raw::syscall1(nr::SCHED_GETSCHEDULER, id as u64) as i64)
}
#[no_mangle]
pub unsafe extern "C" fn sched_setparam(id: i32, param: *const i32) -> i32 {
    super::syscall_result_to_c_int(raw::syscall2(nr::SCHED_SETPARAM, id as u64, param as u64) as i64)
}
#[no_mangle]
pub unsafe extern "C" fn sched_getparam(id: i32, param: *mut i32) -> i32 {
    super::syscall_result_to_c_int(raw::syscall2(nr::SCHED_GETPARAM, id as u64, param as u64) as i64)
}
#[no_mangle]
pub unsafe extern "C" fn sched_get_priority_min(policy: i32) -> i32 {
    super::syscall_result_to_c_int(raw::syscall1(nr::SCHED_GET_PRIORITY_MIN, policy as u64) as i64)
}
#[no_mangle]
pub unsafe extern "C" fn sched_get_priority_max(policy: i32) -> i32 {
    super::syscall_result_to_c_int(raw::syscall1(nr::SCHED_GET_PRIORITY_MAX, policy as u64) as i64)
}
#[no_mangle]
pub unsafe extern "C" fn sched_rr_get_interval(id: i32, t: *mut i64) -> i32 {
    super::syscall_result_to_c_int(
        raw::syscall2(nr::SCHED_RR_GET_INTERVAL, id as u64, t as u64) as i64,
    )
}
#[no_mangle]
pub unsafe extern "C" fn pthread_setschedparam(
    handle: usize,
    policy: i32,
    param: *const i32,
) -> i32 {
    registry_lock();
    let p = find(handle);
    if p.is_null() {
        REGISTRY.unlock();
        return ESRCH;
    }
    let r = raw::syscall3(
        nr::SCHED_SETSCHEDULER,
        (*p).id as u64,
        policy as u64,
        param as u64,
    ) as i64;
    REGISTRY.unlock();
    if r < 0 {
        -r as i32
    } else {
        0
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_getschedparam(
    handle: usize,
    policy: *mut i32,
    param: *mut i32,
) -> i32 {
    if policy.is_null() || param.is_null() {
        return EINVAL;
    }
    registry_lock();
    let p = find(handle);
    if p.is_null() {
        REGISTRY.unlock();
        return ESRCH;
    }
    let r = raw::syscall1(nr::SCHED_GETSCHEDULER, (*p).id as u64) as i64;
    if r < 0 {
        REGISTRY.unlock();
        return -r as i32;
    }
    *policy = r as i32;
    let r = raw::syscall2(nr::SCHED_GETPARAM, (*p).id as u64, param as u64) as i64;
    REGISTRY.unlock();
    if r < 0 {
        -r as i32
    } else {
        0
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_setschedprio(handle: usize, priority: i32) -> i32 {
    registry_lock();
    let p = find(handle);
    if p.is_null() {
        REGISTRY.unlock();
        return ESRCH;
    }
    let r = raw::syscall2(
        nr::SCHED_SETPARAM,
        (*p).id as u64,
        &priority as *const i32 as u64,
    ) as i64;
    REGISTRY.unlock();
    if r < 0 {
        -r as i32
    } else {
        0
    }
}
