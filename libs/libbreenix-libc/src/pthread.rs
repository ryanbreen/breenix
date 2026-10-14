//! C thread interfaces using the ABI storage declared for Breenix clients.
//! Waiters use futexes; a thread's memory is reclaimed once the kernel clears
//! its CLONE_CHILD_CLEARTID word.
mod atfork;
mod cancel;
mod spin;
pub(crate) use atfork::{atfork_child, atfork_parent, atfork_prepare};
pub(crate) use cancel::{cancel_syscall, reserved_block};

use super::{EBUSY, EINVAL, ENOMEM, EPERM};
use core::ptr::{null, null_mut};
use core::sync::atomic::{
    AtomicBool, AtomicPtr, AtomicU32, AtomicUsize,
    Ordering::{AcqRel, Acquire, Relaxed, Release},
};
use libbreenix::syscall::{nr, raw};
const ESRCH: i32 = 3;
const EINTR: i32 = 4;
const EAGAIN: i32 = 11;
const EDEADLK: i32 = 35;
const ETIMEDOUT: i32 = 110;
const EOWNERDEAD: i32 = 130;
const ENOTRECOVERABLE: i32 = 131;
/// Linux UAPI errno value (include/errno.h).
const ENOTSUP: i32 = 95;
const PAGE: usize = 4096;
const KEYS: usize = 128;
const STACK_MIN: usize = 16384;
const FUTEX_WAIT: u64 = 0;
const FUTEX_WAKE: u64 = 1;
const FUTEX_LOCK_PI: u64 = 6;
const FUTEX_UNLOCK_PI: u64 = 7;
const FUTEX_TRYLOCK_PI: u64 = 8;
const FUTEX_WAIT_BITSET: u64 = 9;
const FUTEX_CLOCK_REALTIME: u64 = 256;
const FUTEX_BITSET_MATCH_ANY: u32 = u32::MAX;
const CLONE_VM: u64 = 0x100;
const CLONE_FS: u64 = 0x200;
const CLONE_FILES: u64 = 0x400;
const CLONE_SIGHAND: u64 = 0x800;
const CLONE_THREAD: u64 = 0x10000;
const CLONE_SYSVSEM: u64 = 0x40000;
const CLONE_SETTLS: u64 = 0x80000;
const CLONE_CHILD_CLEARTID: u64 = 0x200000;
const CLONE_CHILD_SETTID: u64 = 0x1000000;
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
        FUTEX_BITSET_MATCH_ANY as u64,
    ) as i64 as i32
}
unsafe fn wake(p: &AtomicU32, n: u32) {
    futex(p, FUTEX_WAKE, n, null());
}
unsafe fn wait(p: &AtomicU32, v: u32) {
    futex(p, FUTEX_WAIT, v, null());
}

/// Internal locks use the same futex protocol as public locks, without errno or
/// descriptor lookup, so allocation of the first descriptor cannot recurse.
struct Lock(AtomicU32);
impl Lock {
    const fn new() -> Self {
        Self(AtomicU32::new(0))
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
/// The bytes of a pthread_attr_t this library reads, for a copy of one
/// kept past the caller's (`mqueue`'s SIGEV_THREAD registrations).
pub(crate) const ATTR_SIZE: usize = core::mem::size_of::<Attr>();
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
/// A thread's descriptor. It lives in the thread's control area, beside its
/// static TLS, at the top of the one mapping that also holds the stack this
/// library allocated for it.
#[repr(C)]
struct Thread {
    next: *mut Thread,
    id: AtomicU32,
    // The kernel's CLONE_CHILD_SETTID/CLEARTID word: the thread ID while the
    // thread runs, 0 once it has ended and no longer uses its memory.
    clear: AtomicU32,
    // 0 joinable, 1 detached, 2 join claimed; under REGISTRY.
    state: u32,
    // Set while pthread_create still uses the descriptor; reaping waits.
    creating: bool,
    // Creation handshake for an explicitly scheduled thread: 0 wait, 1 run,
    // 2 end without running.
    ready: AtomicU32,
    result: *mut u8,
    start: Option<Start>,
    arg: *mut u8,
    attr: Attr,
    mapping: *mut u8,
    mapping_len: usize,
    tp: usize,
    errno: i32,
    locale: *mut core::ffi::c_void,
    cancel: AtomicU32,
    cleanup: *mut cancel::Cleanup,
    robust: *mut Mutex,
    // How many PTHREAD_PRIO_PROTECT mutexes with each ceiling it holds.
    ceilings: [u32; CEILINGS],
    values: [(*mut u8, u32); KEYS],
    extra_values: *mut Values,
    reads: *mut ReadBlock,
}
static REGISTRY: Lock = Lock::new();
static PROCESS: AtomicU32 = AtomicU32::new(0);
static RUNTIME_READY: AtomicBool = AtomicBool::new(false);
static mut EARLY_ERRNO: i32 = 0;
static mut HEAD: *mut Thread = null_mut();
static mut TLS_IMAGE: *const u8 = null();
static mut TLS_FILE: usize = 0;
static mut TLS_SIZE: usize = 0;
static mut TLS_ALIGN: usize = 16;
fn rounded(n: usize, a: usize) -> Option<usize> {
    n.checked_add(a - 1).map(|v| v & !(a - 1))
}

/// Bytes of a thread's control area: its descriptor, then static TLS with
/// room to align it and the ABI's reserved words.
unsafe fn control_len() -> Option<usize> {
    let align = TLS_ALIGN.max(16);
    rounded(core::mem::size_of::<Thread>(), 16)?
        .checked_add(TLS_SIZE)?
        .checked_add(align * 2)?
        .checked_add(32)
        .and_then(|v| rounded(v, PAGE))
}

/// Map `guard` inaccessible bytes, `stack` bytes of stack and the thread's
/// control area above it, and initialize the descriptor and static TLS.
unsafe fn alloc_thread(guard: usize, stack: usize) -> *mut Thread {
    let Some(len) = control_len()
        .and_then(|c| c.checked_add(stack))
        .and_then(|v| v.checked_add(guard))
    else {
        return null_mut();
    };
    let base = map(len, 3);
    if base.is_null() {
        return null_mut();
    }
    if guard != 0
        && (raw::syscall3(nr::MPROTECT, base as u64, guard as u64, 0) as i64) < 0
    {
        unmap(base, len);
        return null_mut();
    }
    let p = base.add(guard + stack) as *mut Thread;
    core::ptr::write(
        p,
        Thread {
            next: null_mut(),
            id: AtomicU32::new(0),
            clear: AtomicU32::new(0),
            state: 0,
            creating: false,
            ready: AtomicU32::new(1),
            result: null_mut(),
            start: None,
            arg: null_mut(),
            attr: Attr::new(),
            mapping: base,
            mapping_len: len,
            tp: 0,
            errno: 0,
            locale: null_mut(),
            cancel: AtomicU32::new(0),
            cleanup: null_mut(),
            robust: null_mut(),
            ceilings: [0; CEILINGS],
            values: [(null_mut(), 0); KEYS],
            extra_values: null_mut(),
            reads: null_mut(),
        },
    );
    let align = TLS_ALIGN.max(16);
    let block = p as usize + rounded(core::mem::size_of::<Thread>(), 16).unwrap();
    #[cfg(target_arch = "aarch64")]
    let (tp, data) = {
        // AArch64 TLS variant I: two reserved words before aligned static TLS.
        let tp = rounded(block, align).unwrap();
        (tp, rounded(tp + 16, align).unwrap())
    };
    #[cfg(target_arch = "x86_64")]
    let (tp, data) = {
        // x86 TLS variant II: the linker places static TLS at TP minus its
        // size rounded to the segment's own alignment. TP is aligned for both
        // that segment and the control words above it.
        let image = rounded(TLS_SIZE, TLS_ALIGN).unwrap();
        let tp = rounded(block + image, align).unwrap();
        (tp, tp - image)
    };
    if TLS_FILE != 0 {
        core::ptr::copy_nonoverlapping(TLS_IMAGE, data as *mut u8, TLS_FILE);
    }
    #[cfg(target_arch = "aarch64")]
    {
        *(tp as *mut usize) = p as usize;
    }
    #[cfg(target_arch = "x86_64")]
    {
        // The compiler reads fs:0 as the variant-II TLS base. Keep our
        // descriptor in the following reserved word, outside static TLS.
        *(tp as *mut usize) = tp;
        *((tp + 8) as *mut usize) = p as usize;
    }
    (*p).tp = tp;
    p
}
unsafe fn free_thread(p: *mut Thread) {
    let mapping = (*p).mapping;
    let len = (*p).mapping_len;
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
    unmap(mapping, len);
}
unsafe fn set_tp(tp: usize) {
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!("msr tpidr_el0, {}", in(reg) tp, options(nostack));
    #[cfg(target_arch = "x86_64")]
    {
        raw::syscall2(nr::ARCH_PRCTL, 0x1002, tp as u64);
    }
}

// The linker defines __ehdr_start when the ELF header is part of a loaded
// segment; a program linked without that leaves the weak reference 0.
core::arch::global_asm!(".weak __ehdr_start");
unsafe fn ehdr_start() -> usize {
    let p: usize;
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!(
        "adrp {p}, __ehdr_start",
        "add {p}, {p}, :lo12:__ehdr_start",
        p = out(reg) p,
        options(nomem, nostack, pure)
    );
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "lea {p}, [rip + __ehdr_start]",
        p = out(reg) p,
        options(nomem, nostack, pure)
    );
    p
}

/// The initial thread's stack as the kernel lays it out: the 16 AT_RANDOM
/// bytes end at its top (else the page holding the environment's terminator
/// does), and it grows down to RLIMIT_STACK within the kernel's 2 MiB growth
/// ceiling. Nothing below it is reserved as a guard.
unsafe fn initial_stack(random: usize, env_end: usize) -> Attr {
    const RLIMIT_STACK: u64 = 3;
    const GROWTH_CEILING: usize = 2 * 1024 * 1024;
    let top = if random != 0 {
        rounded(random + 16, PAGE).unwrap()
    } else {
        rounded(env_end + 8, PAGE).unwrap()
    };
    let mut limit = [0u64; 2];
    let r = raw::syscall4(nr::PRLIMIT64, 0, RLIMIT_STACK, 0, limit.as_mut_ptr() as u64) as i64;
    let size = if r < 0 {
        GROWTH_CEILING
    } else {
        (limit[0].min(GROWTH_CEILING as u64) as usize & !(PAGE - 1)).min(top)
    };
    let mut a = Attr::new();
    a.stack = (top - size) as *mut u8;
    a.size = size;
    a.guard = 0;
    a
}

/// Read the executable's PT_TLS template from its program headers, set up the
/// initial thread's descriptor and TLS, and install its thread pointer. No
/// compiler TLS is accessed before this runs from libc's process entry.
pub unsafe fn startup(envp: *const *const u8) {
    let mut e = envp;
    while !(*e).is_null() {
        e = e.add(1);
    }
    let mut aux = e.add(1) as *const usize;
    let (mut phdr, mut count, mut stride) = (0usize, 0usize, 0usize);
    let mut random = 0usize;
    while *aux != 0 {
        match *aux {
            3 => phdr = *aux.add(1),
            4 => stride = *aux.add(1),
            5 => count = *aux.add(1),
            25 => random = *aux.add(1),
            _ => {}
        }
        aux = aux.add(2);
    }
    // A process started without AT_PHDR finds its headers through the loaded
    // ELF header instead (e_phoff, e_phentsize, e_phnum).
    let mut base_vaddr = None;
    if phdr == 0 {
        let ehdr = ehdr_start();
        if ehdr != 0 {
            let h = ehdr as *const u8;
            phdr = ehdr + core::ptr::read_unaligned(h.add(32) as *const u64) as usize;
            stride = core::ptr::read_unaligned(h.add(54) as *const u16) as usize;
            count = core::ptr::read_unaligned(h.add(56) as *const u16) as usize;
            base_vaddr = Some(ehdr);
        }
    }
    if phdr != 0 && stride >= 56 {
        // The load bias: from PT_PHDR when the headers name themselves, else
        // from the segment that maps file offset 0 (the ELF header).
        let mut bias = 0usize;
        for i in 0..count {
            let h = (phdr + i * stride) as *const u8;
            let kind = core::ptr::read_unaligned(h as *const u32);
            let offset = core::ptr::read_unaligned(h.add(8) as *const usize);
            let vaddr = core::ptr::read_unaligned(h.add(16) as *const usize);
            if kind == 6 {
                bias = phdr.wrapping_sub(vaddr);
            } else if kind == 1 && offset == 0 {
                if let Some(ehdr) = base_vaddr {
                    bias = ehdr.wrapping_sub(vaddr);
                }
            }
        }
        for i in 0..count {
            let h = (phdr + i * stride) as *const u8;
            if core::ptr::read_unaligned(h as *const u32) == 7 {
                TLS_IMAGE = core::ptr::read_unaligned(h.add(16) as *const usize).wrapping_add(bias)
                    as *const u8;
                TLS_FILE = core::ptr::read_unaligned(h.add(32) as *const usize);
                TLS_SIZE = core::ptr::read_unaligned(h.add(40) as *const usize);
                TLS_ALIGN = core::ptr::read_unaligned(h.add(48) as *const usize).max(1);
            }
        }
    }
    let t = alloc_thread(0, 0);
    if t.is_null() {
        super::exit_group(127);
    }
    (*t).attr = initial_stack(random, e as usize);
    let id = tid();
    (*t).id.store(id, Relaxed);
    (*t).clear.store(id, Relaxed);
    raw::syscall1(nr::SET_TID_ADDRESS, &(*t).clear as *const AtomicU32 as u64);
    HEAD = t;
    PROCESS.store(pid(), Relaxed);
    set_tp((*t).tp);
    RUNTIME_READY.store(true, Release);
    cancel::install();
}

/// Repair the registry in a fork child before it enters the C runtime again.
/// Only the calling thread survives a fork, so no inherited lock has an owner
/// in the child; the survivor keeps its descriptor, TLS and thread-specific
/// data. Idempotent: both libc's fork and libbreenix's call it.
#[no_mangle]
pub unsafe extern "C" fn __breenix_after_fork() {
    let process = pid();
    if PROCESS.load(Acquire) == process || !RUNTIME_READY.load(Acquire) {
        return;
    }
    let survivor = descriptor_from_tp();
    REGISTRY.0.store(0, Relaxed);
    KEY_LOCK.0.store(0, Relaxed);
    HEAD = survivor;
    if !survivor.is_null() {
        let id = tid();
        (*survivor).next = null_mut();
        (*survivor).id.store(id, Relaxed);
        (*survivor).clear.store(id, Relaxed);
        (*survivor).state = 0;
        raw::syscall1(
            nr::SET_TID_ADDRESS,
            &(*survivor).clear as *const AtomicU32 as u64,
        );
    }
    PROCESS.store(process, Release);
    crate::mqueue::after_fork();
}
unsafe fn current_id() -> u32 {
    let p = current();
    if p.is_null() {
        tid()
    } else {
        (*p).id.load(Relaxed)
    }
}
/// Reclaim detached threads the kernel has finished with. Under REGISTRY.
unsafe fn reap() {
    let mut link = core::ptr::addr_of_mut!(HEAD);
    while !(*link).is_null() {
        let p = *link;
        if (*p).state == 1 && !(*p).creating && (*p).clear.load(Acquire) == 0 {
            *link = (*p).next;
            free_thread(p);
        } else {
            link = core::ptr::addr_of_mut!((*p).next);
        }
    }
}
unsafe fn unlink(p: *mut Thread) {
    let mut link = core::ptr::addr_of_mut!(HEAD);
    while !(*link).is_null() && *link != p {
        link = core::ptr::addr_of_mut!((**link).next);
    }
    if *link == p {
        *link = (*p).next;
    }
}
unsafe fn current() -> *mut Thread {
    if RUNTIME_READY.load(Acquire) {
        let p = descriptor_from_tp();
        if !p.is_null() {
            return p;
        }
    }
    // A thread this library did not create (raw clone without a thread
    // pointer) gets a descriptor on first use; its thread pointer is its own.
    REGISTRY.lock();
    let id = tid();
    let mut p = HEAD;
    while !p.is_null() {
        if (*p).id.load(Relaxed) == id {
            REGISTRY.unlock();
            return p;
        }
        p = (*p).next;
    }
    p = alloc_thread(0, 0);
    if !p.is_null() {
        // Its stack is its creator's; no bounds are known to report.
        (*p).attr.size = 0;
        (*p).attr.guard = 0;
        (*p).id.store(id, Relaxed);
        // Its creator owns its exit notification; the word reads it running.
        (*p).clear.store(id, Relaxed);
        (*p).next = HEAD;
        HEAD = p;
    }
    REGISTRY.unlock();
    p
}
/// The ABI TCB has a reserved descriptor word. Reading
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
        core::arch::asm!("mov {}, fs:[8]", out(reg) p, options(readonly, nostack));
        p
    }
}

#[no_mangle]
pub extern "C" fn __errno_location() -> *mut i32 {
    unsafe {
        if !RUNTIME_READY.load(Acquire) {
            // Only the initial thread exists before startup completes.
            return core::ptr::addr_of_mut!(EARLY_ERRNO);
        }
        let p = current();
        // Failure to allocate even the thread's C runtime state is fatal rather
        // than silently sharing errno with another thread.
        if p.is_null() {
            super::exit_group(127);
        }
        core::ptr::addr_of_mut!((*p).errno)
    }
}
/// Shared error accessors for the clean-room libc modules.
#[allow(dead_code)] // Published cross-lane API; consumers land independently.
pub(crate) fn set_errno(e: i32) {
    unsafe { *__errno_location() = e; }
}
#[allow(dead_code)] // Published cross-lane API; consumers land independently.
pub(crate) fn errno() -> i32 {
    unsafe { *__errno_location() }
}

/// A null slot selects the process locale; a new thread starts in that mode.
#[allow(dead_code)] // Published cross-lane API for the locale owner.
pub(crate) unsafe fn locale_slot() -> *mut *mut core::ffi::c_void {
    let p = current();
    if p.is_null() { super::exit_group(127); }
    core::ptr::addr_of_mut!((*p).locale)
}

/// Recursive FILE lock. The owner is the holder's thread descriptor address:
/// reading it needs no syscall, and a fork child keeps the forking thread's
/// descriptor, so a lock that thread held (flockfile before fork) is still
/// its own in the child. `state` is the futex word: 0 free, 1 held, 2 held
/// with waiters, so an unlock with no waiters makes no syscall. Only the
/// owner accesses depth, between acquire and release operations on state.
#[allow(dead_code)] // Published cross-lane API for the stdio owner.
pub(crate) struct RecursiveLock {
    owner: AtomicUsize,
    state: AtomicU32,
    depth: core::cell::UnsafeCell<usize>,
}
unsafe impl Sync for RecursiveLock {}
/// The calling thread's lock-owner identity: its descriptor address, or 1
/// before startup, when only the initial thread exists.
fn lock_owner() -> usize {
    unsafe {
        if RUNTIME_READY.load(Acquire) {
            let p = current();
            if !p.is_null() {
                return p as usize;
            }
        }
    }
    1
}
#[allow(dead_code)] // Published cross-lane API for the stdio owner.
impl RecursiveLock {
    pub(crate) const fn new() -> Self {
        Self {
            owner: AtomicUsize::new(0),
            state: AtomicU32::new(0),
            depth: core::cell::UnsafeCell::new(0),
        }
    }
    fn held(&self, me: usize) {
        self.owner.store(me, Relaxed);
        unsafe { *self.depth.get() = 1; }
    }
    pub(crate) fn try_lock(&self) -> bool {
        let me = lock_owner();
        // Only this thread ever stores its own identity here.
        if self.owner.load(Relaxed) == me {
            unsafe { *self.depth.get() += 1; }
            return true;
        }
        if self.state.compare_exchange(0, 1, Acquire, Relaxed).is_err() {
            return false;
        }
        self.held(me);
        true
    }
    pub(crate) fn lock(&self) {
        if self.try_lock() {
            return;
        }
        while self.state.swap(2, Acquire) != 0 {
            unsafe { wait(&self.state, 2); }
        }
        self.held(lock_owner());
    }
    pub(crate) fn unlock(&self) {
        unsafe {
            *self.depth.get() -= 1;
            if *self.depth.get() == 0 {
                self.owner.store(0, Relaxed);
                if self.state.swap(0, Release) == 2 {
                    wake(&self.state, 1);
                }
            }
        }
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
        // CLONE_SETTLS installed the thread pointer and CLONE_CHILD_SETTID
        // wrote this thread's ID before it was scheduled.
        (*p).id.store((*p).clear.load(Relaxed), Relaxed);
        cancel::thread_start(p);
        if (*p).ready.load(Acquire) != 1 {
            loop {
                let r = (*p).ready.load(Acquire);
                if r != 0 {
                    break;
                }
                wait(&(*p).ready, 0);
            }
            if (*p).ready.load(Acquire) == 2 {
                libbreenix::process::exit(0);
            }
        }
        let result = ((*p).start.unwrap())((*p).arg);
        pthread_exit(result)
    }
}

/// Wait until the kernel has finished with thread `p`.
unsafe fn await_end(p: *mut Thread) {
    loop {
        let id = (*p).clear.load(Acquire);
        if id == 0 {
            break;
        }
        wait(&(*p).clear, id);
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
        return EAGAIN;
    }
    let a = if attr.is_null() {
        Attr::new()
    } else {
        *(attr as *const Attr)
    };
    REGISTRY.lock();
    reap();
    REGISTRY.unlock();
    // The kernel starts a thread with its creator's policy, which is
    // PTHREAD_INHERIT_SCHED. An explicit policy is applied before it runs.
    let explicit = a.inherit == 1;
    let p = if a.stack.is_null() {
        let (Some(guard), Some(size)) = (rounded(a.guard, PAGE), rounded(a.size, PAGE)) else {
            return EINVAL;
        };
        let p = alloc_thread(guard, size);
        if !p.is_null() {
            (*p).attr.stack = (*p).mapping.add(guard);
        }
        p
    } else {
        let p = alloc_thread(0, 0);
        if !p.is_null() {
            (*p).attr.stack = a.stack;
        }
        p
    };
    if p.is_null() {
        return EAGAIN;
    }
    let stack = (*p).attr.stack;
    let Some(top) = (stack as usize).checked_add(a.size) else {
        free_thread(p);
        return EINVAL;
    };
    (*p).attr.size = a.size;
    (*p).attr.guard = a.guard;
    (*p).attr.detached = a.detached;
    (*p).attr.inherit = a.inherit;
    (*p).attr.policy = a.policy;
    (*p).attr.priority = a.priority;
    (*p).start = Some(start);
    (*p).arg = arg;
    (*p).state = a.detached as u32;
    (*p).creating = true;
    (*p).ready.store(if explicit { 0 } else { 1 }, Relaxed);
    cancel::creating(p);
    // Nonzero until the kernel reports the thread ended, so neither reaping
    // nor a join can take it before it has started.
    (*p).clear.store(u32::MAX, Relaxed);
    // Published before the thread exists, so the thread itself can detach or
    // name itself at once.
    REGISTRY.lock();
    (*p).next = HEAD;
    HEAD = p;
    REGISTRY.unlock();
    // `entry` starts as if called: AArch64 keeps SP 16-byte aligned, and the
    // AMD64 ABI wants RSP+8 aligned at function entry, below a return
    // address slot (zero, ending backtraces).
    #[cfg(target_arch = "aarch64")]
    let sp = top & !15;
    #[cfg(target_arch = "x86_64")]
    let sp = {
        let sp = (top & !15) - 8;
        *(sp as *mut usize) = 0;
        sp
    };
    // CLONE_SYSVSEM: a process's threads share its semadj values, as POSIX
    // keeps them per process.
    let flags = CLONE_VM
        | CLONE_FS
        | CLONE_FILES
        | CLONE_SIGHAND
        | CLONE_THREAD
        | CLONE_SYSVSEM
        | CLONE_SETTLS
        | CLONE_CHILD_CLEARTID
        | CLONE_CHILD_SETTID;
    let r = raw::syscall6(
        nr::CLONE,
        flags,
        sp as u64,
        entry as *const () as u64,
        p as u64,
        &(*p).clear as *const AtomicU32 as u64,
        (*p).tp as u64,
    ) as i64;
    if r < 0 {
        REGISTRY.lock();
        unlink(p);
        REGISTRY.unlock();
        free_thread(p);
        return if r == -(EPERM as i64) {
            EPERM
        } else if r == -(EINVAL as i64) {
            EINVAL
        } else {
            EAGAIN
        };
    }
    (*p).id.store(r as u32, Relaxed);
    if explicit {
        let result = raw::syscall3(
            nr::SCHED_SETSCHEDULER,
            r as u64,
            a.policy as u64,
            &a.priority as *const i32 as u64,
        ) as i64;
        if result < 0 {
            // A failed creation leaves no thread for the caller to collect.
            // The child never ran the application routine.
            (*p).ready.store(2, Release);
            wake(&(*p).ready, 1);
            await_end(p);
            REGISTRY.lock();
            unlink(p);
            REGISTRY.unlock();
            free_thread(p);
            return if result == -(EPERM as i64) {
                EPERM
            } else {
                EINVAL
            };
        }
        (*p).ready.store(1, Release);
        wake(&(*p).ready, 1);
    }
    *out = p as usize;
    // From here a detached thread that has ended may be reclaimed at once,
    // including one a join handed back while this call still used it.
    REGISTRY.lock();
    (*p).creating = false;
    reap();
    REGISTRY.unlock();
    0
}
unsafe extern "C" fn release_join(arg: *mut u8) {
    REGISTRY.lock();
    (*(arg as *mut Thread)).state = 0;
    REGISTRY.unlock();
}
#[no_mangle]
pub unsafe extern "C" fn pthread_join(handle: usize, value: *mut *mut u8) -> i32 {
    if handle == pthread_self() {
        return EDEADLK;
    }
    cancel::pthread_testcancel();
    REGISTRY.lock();
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
    let mut cleanup = cancel::Cleanup::new();
    cancel::__breenix_cleanup_push(&mut cleanup, release_join, p as *mut u8);
    loop {
        let id = (*p).clear.load(Acquire);
        if id == 0 { break; }
        cancel_syscall(nr::FUTEX, [&(*p).clear as *const AtomicU32 as u64,
            FUTEX_WAIT, id as u64, 0, 0, 0]);
    }
    cancel::__breenix_cleanup_pop(&mut cleanup, 0);
    if !value.is_null() {
        *value = (*p).result;
    }
    REGISTRY.lock();
    if (*p).creating {
        // The thread published itself and ended before pthread_create
        // finished with its descriptor; that call reclaims it as detached.
        (*p).state = 1;
        REGISTRY.unlock();
        return 0;
    }
    unlink(p);
    REGISTRY.unlock();
    free_thread(p);
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_detach(handle: usize) -> i32 {
    REGISTRY.lock();
    let p = find(handle);
    if p.is_null() {
        REGISTRY.unlock();
        return ESRCH;
    }
    if (*p).state != 0 {
        REGISTRY.unlock();
        return EINVAL;
    }
    (*p).state = 1;
    // A thread that has already ended is reclaimed now; a running one by the
    // first registry pass after the kernel clears its exit word.
    reap();
    REGISTRY.unlock();
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_exit(value: *mut u8) -> ! {
    let p = current();
    if !p.is_null() {
        cancel::finish(p);
        destructors(p);
        while !(*p).robust.is_null() {
            let m = (*p).robust;
            (*p).robust = (*m).next;
            (*m).recovery.store(1, Relaxed);
            (*m).depth = 0;
            if protocol((*m).kind) == PRIO_INHERIT {
                pi_release(m);
            } else {
                (*m).owner.store(0, Release);
                wake(&(*m).owner, u32::MAX);
            }
        }
        (*p).result = value;
    }
    // The kernel ends the process when its last thread ends, with the status
    // of exit(0) when the initial thread left through pthread_exit.
    libbreenix::process::exit(0)
}

#[no_mangle]
pub unsafe extern "C" fn pthread_getattr_np(handle: usize, attr: *mut u8) -> i32 {
    if attr.is_null() {
        return EINVAL;
    }
    REGISTRY.lock();
    let p = find(handle);
    if p.is_null() {
        REGISTRY.unlock();
        return ESRCH;
    }
    *(attr as *mut Attr) = (*p).attr;
    (*(attr as *mut Attr)).detached = ((*p).state == 1) as i32;
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
    generation: AtomicU32,
    destructor: Destructor,
}
#[repr(C)]
struct KeyBlock {
    next: AtomicPtr<KeyBlock>,
    keys: [Key; KEYS],
}
#[repr(C)]
struct Values {
    next: *mut Values,
    block: u32,
    entries: [(*mut u8, u32); KEYS],
}
static KEY_LOCK: Lock = Lock::new();
static KEY_BLOCKS: AtomicPtr<KeyBlock> = AtomicPtr::new(null_mut());
unsafe fn key_at(k: u32) -> *mut Key {
    let mut p = KEY_BLOCKS.load(Acquire);
    for _ in 0..k as usize / KEYS {
        if p.is_null() {
            return null_mut();
        }
        p = (*p).next.load(Acquire);
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
    let mut link = &KEY_BLOCKS;
    let mut base = 0u32;
    loop {
        let mut block = link.load(Relaxed);
        if block.is_null() {
            block = map(PAGE, 3) as *mut KeyBlock;
            if block.is_null() {
                KEY_LOCK.unlock();
                return EAGAIN;
            }
            core::ptr::write(
                block,
                KeyBlock {
                    next: AtomicPtr::new(null_mut()),
                    keys: core::array::from_fn(|_| Key {
                        generation: AtomicU32::new(0),
                        destructor: None,
                    }),
                },
            );
            link.store(block, Release);
        }
        for i in 0..KEYS {
            let key = core::ptr::addr_of_mut!((*block).keys[i]);
            let generation = (*key).generation.load(Relaxed);
            if generation & 1 == 0 {
                (*key).destructor = destructor;
                (*key).generation.store(generation.wrapping_add(1), Release);
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
        link = &(*block).next;
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_key_delete(k: u32) -> i32 {
    KEY_LOCK.lock();
    let key = key_at(k);
    if key.is_null() || (*key).generation.load(Relaxed) & 1 == 0 {
        KEY_LOCK.unlock();
        return EINVAL;
    }
    (*key).generation.fetch_add(1, Release);
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
    if key.is_null() || (*key).generation.load(Relaxed) & 1 == 0 {
        KEY_LOCK.unlock();
        return EINVAL;
    }
    let entry = value_at(p, k, true);
    if entry.is_null() {
        KEY_LOCK.unlock();
        return ENOMEM;
    }
    *entry = (value, (*key).generation.load(Relaxed));
    KEY_LOCK.unlock();
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_getspecific(k: u32) -> *mut u8 {
    let p = current();
    if p.is_null() {
        return null_mut();
    }
    let key = key_at(k);
    let entry = value_at(p, k, false);
    let generation = if key.is_null() {
        0
    } else {
        (*key).generation.load(Acquire)
    };
    let value =
        if !key.is_null() && !entry.is_null() && generation & 1 != 0 && (*entry).1 == generation {
            (*entry).0
        } else {
            null_mut()
        };
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
                    if g & 1 != 0 && g == (*definition).generation.load(Relaxed) {
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
fn clock_now(clock: i32) -> [i64; 2] {
    let mut t = [0i64; 2];
    unsafe { raw::syscall2(nr::CLOCK_GETTIME, clock as u64, t.as_mut_ptr() as u64); }
    t
}
/// The CLOCK_REALTIME time at which CLOCK_MONOTONIC reads `t`.
fn monotonic_to_realtime(t: [i64; 2]) -> [i64; 2] {
    let (mono, real) = (clock_now(1), clock_now(0));
    let ns = |v: [i64; 2]| v[0] as i128 * 1_000_000_000 + v[1] as i128;
    let at = (ns(t) - ns(mono) + ns(real)).max(0);
    [(at / 1_000_000_000) as i64, (at % 1_000_000_000) as i64]
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
    // A deadline before either clock's epoch has passed already; the kernel
    // rejects negative seconds.
    if t[0] < 0 {
        return ETIMEDOUT;
    }
    // FUTEX_WAIT_BITSET takes an absolute deadline on CLOCK_MONOTONIC, or on
    // CLOCK_REALTIME (following clock changes) with FUTEX_CLOCK_REALTIME.
    let op = FUTEX_WAIT_BITSET | if clock == 0 { FUTEX_CLOCK_REALTIME } else { 0 };
    let r = futex(p, op, value, t.as_ptr());
    if r == -ETIMEDOUT {
        ETIMEDOUT
    } else if r == -EINVAL {
        EINVAL
    } else {
        0
    }
}

// A mutex's owner is also its futex word; the high bit carries the wake baton
// through contended acquisitions, avoiding a syscall on uncontended unlock.
// `kind` is the attribute the mutex was made with and never changes after.
#[repr(C)]
struct Mutex {
    owner: AtomicU32,
    kind: u32,
    depth: u32,
    recovery: AtomicU32, // 0 healthy, 1 owner dead, 2 not recoverable, STALLED
    ceiling: AtomicU32,  // the priority ceiling, changed only by an owner
    held: u32,           // private to the owner: the ceiling it runs at for this mutex
    next: *mut Mutex,    // private to the owner, used only for robust mutexes
}
const ROBUST: u32 = 4;
/// PTHREAD_PROCESS_SHARED in a mutex attribute.
const MUTEX_SHARED: u32 = 1 << 29;
const WAITERS: u32 = 1 << 31;
// A PTHREAD_MUTEX_STALLED PTHREAD_PRIO_INHERIT mutex whose owner ended holding
// it: its data may be inconsistent, so no thread acquires it again.
const STALLED: u32 = 3;
// A mutex attribute holds the type in bits 0-1, ROBUST, the protocol in bits
// 3-4 and the priority ceiling in bits 8-15 (0: the lowest SCHED_FIFO
// priority).
const PROTOCOL_SHIFT: u32 = 3;
const PROTOCOL_MASK: u32 = 3 << PROTOCOL_SHIFT;
const PRIO_INHERIT: u32 = 1;
const PRIO_PROTECT: u32 = 2;
const CEILING_SHIFT: u32 = 8;
const CEILING_MASK: u32 = 0xff << CEILING_SHIFT;
/// SCHED_FIFO's priorities run 1 to 99.
const CEILINGS: usize = 100;
// A PTHREAD_PRIO_INHERIT mutex's word follows the kernel's
// priority-inheritance futex protocol: its owner's thread ID, WAITERS while
// threads are blocked in the kernel, OWNER_DIED once an owner ended holding it.
const OWNER_DIED: u32 = 1 << 30;
const TID_MASK: u32 = OWNER_DIED - 1;
fn protocol(kind: u32) -> u32 {
    (kind & PROTOCOL_MASK) >> PROTOCOL_SHIFT
}
/// A priority ceiling as an attribute holds it, as a SCHED_FIFO priority.
fn attr_ceiling(attr: u32) -> u32 {
    ((attr & CEILING_MASK) >> CEILING_SHIFT).clamp(1, CEILINGS as u32 - 1)
}
unsafe fn ceiling(m: *const Mutex) -> u32 {
    (*m).ceiling.load(Relaxed).clamp(1, CEILINGS as u32 - 1)
}
unsafe fn mutex_take(m: *mut Mutex, attempt: bool, deadline: *const i64, clock: i32) -> i32 {
    if m.is_null() {
        return EINVAL;
    }
    match protocol((*m).kind) {
        PRIO_INHERIT => pi_take(m, attempt, deadline, clock),
        PRIO_PROTECT => pp_take(m, attempt, deadline, clock),
        _ => plain_take(m, attempt, deadline, clock),
    }
}
/// The mutex has just been taken: finish recording the caller as its owner.
unsafe fn taken(m: *mut Mutex) -> i32 {
    (*m).depth = 1;
    if (*m).kind & ROBUST != 0 {
        let t = current();
        (*m).next = (*t).robust;
        (*t).robust = m;
    }
    if (*m).recovery.load(Relaxed) == 1 {
        EOWNERDEAD
    } else {
        0
    }
}
unsafe fn plain_take(m: *mut Mutex, attempt: bool, deadline: *const i64, clock: i32) -> i32 {
    let id = current_id();
    let mut contended = 0;
    loop {
        if (*m).recovery.load(Acquire) == 2 {
            return ENOTRECOVERABLE;
        }
        match (*m)
            .owner
            .compare_exchange(0, id | contended, Acquire, Relaxed)
        {
            Ok(_) => {
                // A recovering owner can mark the mutex unrecoverable between
                // our initial check and acquisition. Never admit that race.
                if (*m).recovery.load(Acquire) == 2 {
                    let owner = core::ptr::addr_of!((*m).owner);
                    (*owner).store(0, Release);
                    futex(owner, FUTEX_WAKE, u32::MAX, null());
                    return ENOTRECOVERABLE;
                }
                return taken(m);
            }
            Err(owner) => {
                if owner & !WAITERS == id {
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
                contended = WAITERS;
                if (*m)
                    .owner
                    .compare_exchange(owner, owner | WAITERS, Relaxed, Relaxed)
                    .is_err()
                {
                    continue;
                }
                let r = deadline_wait(&(*m).owner, owner | WAITERS, deadline, clock);
                if r != 0 {
                    return r;
                }
            }
        }
    }
}
/// Lock a PTHREAD_PRIO_INHERIT mutex: uncontended in userspace, otherwise
/// in the kernel, which runs its owner at the caller's priority meanwhile.
unsafe fn pi_take(m: *mut Mutex, attempt: bool, deadline: *const i64, clock: i32) -> i32 {
    let id = current_id();
    let word = core::ptr::addr_of!((*m).owner);
    loop {
        match (*m).recovery.load(Acquire) {
            2 => return ENOTRECOVERABLE,
            // Never acquired again: a lock waits for ever, a timed lock until
            // its deadline, and a try fails.
            STALLED => {
                if attempt {
                    return EBUSY;
                }
                let r = deadline_wait(&(*m).recovery, STALLED, deadline, clock);
                if r != 0 {
                    return r;
                }
                continue;
            }
            _ => {}
        }
        if (*word).load(Relaxed) & TID_MASK == id {
            if (*m).kind & 3 == 1 {
                let Some(depth) = (*m).depth.checked_add(1) else {
                    return EAGAIN;
                };
                (*m).depth = depth;
                return 0;
            }
            return if attempt { EBUSY } else { EDEADLK };
        }
        if (*word).compare_exchange(0, id, Acquire, Relaxed).is_err() {
            let r = if attempt {
                futex(word, FUTEX_TRYLOCK_PI, 0, null())
            } else {
                let mut realtime = [0i64; 2];
                if !deadline.is_null() {
                    let t = core::ptr::read_unaligned(deadline as *const [i64; 2]);
                    if !valid_time(&t) {
                        return EINVAL;
                    }
                    if t[0] < 0 {
                        return ETIMEDOUT;
                    }
                    realtime = if clock == 0 { t } else { monotonic_to_realtime(t) };
                }
                // FUTEX_LOCK_PI takes an absolute CLOCK_REALTIME deadline; a
                // CLOCK_MONOTONIC one is converted at the clocks' current offset.
                futex(word, FUTEX_LOCK_PI, 0,
                    if deadline.is_null() { null() } else { realtime.as_ptr() })
            };
            match -r {
                0 => {}
                EINTR => continue,
                EAGAIN | EDEADLK if attempt => return EBUSY,
                e => return e,
            }
        }
        // An owner that ended holding it left OWNER_DIED for this one. A
        // robust mutex reports it (EOWNERDEAD); any other stalls.
        if (*word).fetch_and(!OWNER_DIED, Relaxed) & OWNER_DIED != 0 {
            if (*m).kind & ROBUST != 0 {
                let _ = (*m).recovery.compare_exchange(0, 1, Relaxed, Relaxed);
            } else {
                (*m).recovery.store(STALLED, Release);
            }
        }
        match (*m).recovery.load(Acquire) {
            2 => {
                pi_release(m);
                return ENOTRECOVERABLE;
            }
            // Pass it on, so a thread handed it in the kernel stalls too.
            STALLED => {
                pi_release(m);
                continue;
            }
            _ => return taken(m),
        }
    }
}
/// Release a PTHREAD_PRIO_INHERIT mutex the caller owns: in userspace when
/// no thread is blocked on it, otherwise the kernel hands it to the
/// highest-priority waiter. No object read follows release.
unsafe fn pi_release(m: *mut Mutex) {
    let word = core::ptr::addr_of!((*m).owner);
    if (*word).compare_exchange(current_id(), 0, Release, Relaxed).is_err() {
        futex(word, FUTEX_UNLOCK_PI, 0, null());
    }
}
/// Run the calling thread at no less than the highest ceiling among the
/// PTHREAD_PRIO_PROTECT mutexes it holds; with `check`, refuse (EINVAL) if
/// its own priority is above `check`. EPERM if it may not take a real-time
/// priority that high.
unsafe fn apply_ceilings(t: *mut Thread, check: u32) -> i32 {
    let top = (1..CEILINGS).rev().find(|&c| (*t).ceilings[c] > 0).unwrap_or(0);
    let r = raw::syscall2(nr::PRIO_CEILING, top as u64, check as u64) as i64;
    if r < 0 {
        -r as i32
    } else {
        0
    }
}
/// The calling thread takes a PTHREAD_PRIO_PROTECT mutex with ceiling `c`:
/// it runs at that ceiling from now on, unless refused (`apply_ceilings`).
unsafe fn raise_ceiling(t: *mut Thread, c: u32) -> i32 {
    (*t).ceilings[c as usize] += 1;
    let r = apply_ceilings(t, c);
    if r != 0 {
        (*t).ceilings[c as usize] -= 1;
    }
    r
}
/// Lock a PTHREAD_PRIO_PROTECT mutex: the caller first rises to its ceiling
/// (EINVAL if its own priority is above the ceiling), and stays there until
/// it unlocks.
unsafe fn pp_take(m: *mut Mutex, attempt: bool, deadline: *const i64, clock: i32) -> i32 {
    if (*m).owner.load(Relaxed) & !WAITERS == current_id() {
        return plain_take(m, attempt, deadline, clock);
    }
    let t = current();
    if t.is_null() {
        return EAGAIN;
    }
    let c = ceiling(m);
    let r = raise_ceiling(t, c);
    if r != 0 {
        return r;
    }
    let r = plain_take(m, attempt, deadline, clock);
    if r != 0 && r != EOWNERDEAD {
        lower_ceiling(c);
        return r;
    }
    // Only an owner changes the ceiling, so it is final now; a waiter took
    // the one it had before.
    let now = ceiling(m);
    if now != c {
        let e = raise_ceiling(t, now);
        lower_ceiling(c);
        if e != 0 {
            untake(m);
            return e;
        }
    }
    (*m).held = now;
    r
}
/// The caller no longer holds a PTHREAD_PRIO_PROTECT mutex it took at
/// ceiling `c`.
unsafe fn lower_ceiling(c: u32) {
    let t = current();
    if !t.is_null() && (*t).ceilings[c as usize] > 0 {
        (*t).ceilings[c as usize] -= 1;
        apply_ceilings(t, 0);
    }
}
/// Remove `m` from the calling thread's list of the robust mutexes it holds.
unsafe fn unlink_robust(m: *mut Mutex) {
    let t = current();
    let mut link = core::ptr::addr_of_mut!((*t).robust);
    while !(*link).is_null() && *link != m {
        link = core::ptr::addr_of_mut!((**link).next);
    }
    if *link == m {
        *link = (*m).next;
    }
}
/// Give back a mutex just taken, leaving it as it was found: an owner's death
/// is still reported to the next locker.
unsafe fn untake(m: *mut Mutex) {
    if (*m).kind & ROBUST != 0 {
        unlink_robust(m);
    }
    (*m).depth = 0;
    let owner = core::ptr::addr_of!((*m).owner);
    if (*owner).swap(0, Release) & WAITERS != 0 {
        futex(owner, FUTEX_WAKE, 1, null());
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
            ceiling: AtomicU32::new(if a.is_null() { 1 } else { attr_ceiling(*(a as *const u32)) }),
            held: 0,
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
    mutex_take(m as *mut Mutex, false, null(), 0)
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_trylock(m: *mut u8) -> i32 {
    mutex_take(m as *mut Mutex, true, null(), 0)
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_timedlock(m: *mut u8, t: *const i64) -> i32 {
    if t.is_null() {
        EINVAL
    } else {
        mutex_take(m as *mut Mutex, false, t, 0)
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_clocklock(m: *mut u8, clock: i32, t: *const i64) -> i32 {
    if t.is_null() || !(0..=1).contains(&clock) {
        EINVAL
    } else {
        mutex_take(m as *mut Mutex, false, t, clock)
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_unlock(m: *mut u8) -> i32 {
    if m.is_null() {
        return EINVAL;
    }
    let m = m as *mut Mutex;
    if (*m).kind != 0 && (*m).owner.load(Relaxed) & !WAITERS != current_id() {
        return EPERM;
    }
    if (*m).kind & 3 == 1 && (*m).depth > 1 {
        (*m).depth -= 1;
        return 0;
    }
    if (*m).kind & ROBUST != 0 {
        unlink_robust(m);
        if (*m).recovery.load(Relaxed) == 1 {
            (*m).recovery.store(2, Relaxed);
        }
    }
    (*m).depth = 0;
    let count = if (*m).recovery.load(Relaxed) == 2 {
        u32::MAX
    } else {
        1
    };
    let protocol = protocol((*m).kind);
    if protocol == PRIO_INHERIT {
        pi_release(m);
        return 0;
    }
    let held = (*m).held;
    let owner = core::ptr::addr_of!((*m).owner);
    if (*owner).swap(0, Release) & WAITERS != 0 {
        // No object read follows release: the next owner may destroy it.
        futex(owner, FUTEX_WAKE, count, null());
    }
    if protocol == PRIO_PROTECT {
        lower_ceiling(held);
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_setprioceiling(m: *mut u8, prio: i32, old: *mut i32) -> i32 {
    if m.is_null() || !(1..CEILINGS as i32).contains(&prio) {
        return EINVAL;
    }
    let m = m as *mut Mutex;
    if protocol((*m).kind) == 0 {
        return EINVAL;
    }
    // The ceiling changes with the mutex locked as pthread_mutex_lock locks
    // it: EDEADLK for an error-checking mutex the caller holds, once more for
    // a recursive one. A robust mutex whose owner died stays locked by the
    // caller, unchanged, for pthread_mutex_consistent.
    let r = mutex_take(m, false, null(), 0);
    if r != 0 {
        return r;
    }
    let previous = ceiling(m);
    (*m).ceiling.store(prio as u32, Relaxed);
    // An owner that goes on holding it runs at the new ceiling from now on.
    if protocol((*m).kind) == PRIO_PROTECT && (*m).depth > 1 && prio as u32 != (*m).held {
        let t = current();
        let e = raise_ceiling(t, prio as u32);
        if e != 0 {
            (*m).ceiling.store(previous, Relaxed);
            pthread_mutex_unlock(m as *mut u8);
            return e;
        }
        lower_ceiling((*m).held);
        (*m).held = prio as u32;
    }
    if !old.is_null() {
        *old = previous as i32;
    }
    pthread_mutex_unlock(m as *mut u8);
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_getprioceiling(m: *const u8, prio: *mut i32) -> i32 {
    if m.is_null() || prio.is_null() || protocol((*(m as *const Mutex)).kind) == 0 {
        return EINVAL;
    }
    *prio = ceiling(m as *const Mutex) as i32;
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_consistent(m: *mut u8) -> i32 {
    if m.is_null() {
        return EINVAL;
    }
    let m = &*(m as *mut Mutex);
    if m.kind & ROBUST == 0
        || m.owner.load(Relaxed) & !WAITERS != current_id()
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
    if v == 1 && *(a as *const u32) & MUTEX_SHARED != 0 {
        return ENOTSUP;
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
pub unsafe extern "C" fn pthread_mutexattr_setprotocol(a: *mut u8, v: i32) -> i32 {
    if a.is_null() || !(0..=2).contains(&v) {
        return EINVAL;
    }
    *(a as *mut u32) = (*(a as *mut u32) & !PROTOCOL_MASK) | ((v as u32) << PROTOCOL_SHIFT);
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_getprotocol(a: *const u8, v: *mut i32) -> i32 {
    if a.is_null() || v.is_null() {
        return EINVAL;
    }
    *v = protocol(*(a as *const u32)) as i32;
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_setprioceiling(a: *mut u8, prio: i32) -> i32 {
    if a.is_null() || !(1..CEILINGS as i32).contains(&prio) {
        return EINVAL;
    }
    *(a as *mut u32) = (*(a as *mut u32) & !CEILING_MASK) | ((prio as u32) << CEILING_SHIFT);
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_getprioceiling(a: *const u8, prio: *mut i32) -> i32 {
    if a.is_null() || prio.is_null() {
        return EINVAL;
    }
    *prio = attr_ceiling(*(a as *const u32)) as i32;
    0
}

/// Set in a waiter count while destroy waits for it to drain, so departing
/// waiters wake the destroyer only when one is waiting.
const DRAIN: u32 = 1 << 31;
/// Leave a drained count; wake a waiting destroyer. No object read follows.
unsafe fn depart(count: *const AtomicU32) {
    if (*count).fetch_sub(1, Release) & DRAIN != 0 {
        futex(count, FUTEX_WAKE, u32::MAX, null());
    }
}
/// Mark a count draining and wait until no user holds it.
fn drain(count: &AtomicU32) {
    count.fetch_or(DRAIN, Acquire);
    loop {
        let n = count.load(Acquire);
        if n & !DRAIN == 0 {
            break;
        }
        unsafe {
            wait(count, n);
        }
    }
}
#[repr(C)]
struct Cond {
    sequence: AtomicU32,
    clock: i32,
    waiting: AtomicU32,
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
            clock: if a.is_null() { 0 } else { *(a as *const i32) & 1 },
            waiting: AtomicU32::new(0),
        },
    );
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_cond_destroy(c: *mut u8) -> i32 {
    if c.is_null() {
        EINVAL
    } else {
        drain(&(*(c as *const Cond)).waiting);
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
struct CondCleanup { cond: *const Cond, mutex: *mut u8, seq: u32 }
/// A waiter cancelled after a signal or broadcast moved the sequence may have
/// taken that signal's futex wake. POSIX.1-2024 (pthread_cond_wait) forbids a
/// cancelled waiter consuming a signal while other threads wait, so pass one
/// wake on (a spurious wakeup for the others is permitted).
unsafe extern "C" fn cancel_cond(arg: *mut u8) {
    let state = &*(arg as *const CondCleanup);
    if (*state.cond).sequence.load(Acquire) != state.seq {
        cond_wake(state.cond as *mut u8, 1);
    }
    depart(core::ptr::addr_of!((*state.cond).waiting));
    pthread_mutex_lock(state.mutex);
}
unsafe fn cond_deadline_wait(p: &AtomicU32, value: u32, deadline: *const i64, clock: i32) -> i32 {
    let op = if deadline.is_null() { FUTEX_WAIT } else {
        FUTEX_WAIT_BITSET | if clock == 0 { FUTEX_CLOCK_REALTIME } else { 0 }
    };
    if !deadline.is_null() && (*deadline) < 0 { cancel::pthread_testcancel(); return ETIMEDOUT; }
    let r = cancel_syscall(nr::FUTEX, [p as *const AtomicU32 as u64, op,
        value as u64, deadline as u64, 0, FUTEX_BITSET_MATCH_ANY as u64]);
    if r == -ETIMEDOUT as i64 { ETIMEDOUT }
    else if r == -EINVAL as i64 { EINVAL }
    else { 0 }
}
unsafe fn cond_wait(c: *mut u8, m: *mut u8, t: *const i64, clock: Option<i32>) -> i32 {
    if c.is_null() || m.is_null() {
        return EINVAL;
    }
    if !t.is_null() && !valid_time(&core::ptr::read_unaligned(t as *const [i64; 2])) {
        return EINVAL;
    }
    let c = &*(c as *const Cond);
    c.waiting.fetch_add(1, Acquire);
    let seq = c.sequence.load(Acquire);
    let clock = clock.unwrap_or(c.clock);
    let r = pthread_mutex_unlock(m);
    if r != 0 {
        depart(core::ptr::addr_of!(c.waiting));
        return r;
    }
    let mut state = CondCleanup { cond: c, mutex: m, seq };
    let mut cleanup = cancel::Cleanup::new();
    cancel::__breenix_cleanup_push(&mut cleanup, cancel_cond, &mut state as *mut _ as *mut u8);
    let mut r;
    loop {
        r = cond_deadline_wait(&c.sequence, seq, t, clock);
        if r != 0 || c.sequence.load(Acquire) != seq {
            break;
        }
    }
    cancel::__breenix_cleanup_pop(&mut cleanup, 0);
    // Destroy drains this reference count before freeing or reinitializing
    // the condition; release it before reacquiring the application mutex.
    depart(core::ptr::addr_of!(c.waiting));
    let lock = pthread_mutex_lock(m);
    if lock != 0 {
        lock
    } else {
        r
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_cond_wait(c: *mut u8, m: *mut u8) -> i32 {
    cond_wait(c, m, null(), None)
}
#[no_mangle]
pub unsafe extern "C" fn pthread_cond_timedwait(c: *mut u8, m: *mut u8, t: *const i64) -> i32 {
    if t.is_null() {
        EINVAL
    } else {
        cond_wait(c, m, t, None)
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_cond_clockwait(c: *mut u8, m: *mut u8, clock: i32, t: *const i64) -> i32 {
    if t.is_null() || !(0..=1).contains(&clock) { EINVAL }
    else { cond_wait(c, m, t, Some(clock)) }
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
    *(a as *mut i32) = (*(a as *mut i32) & !1) | clock;
    0
}
#[no_mangle]
pub unsafe extern "C" fn pthread_condattr_getclock(a: *const u8, clock: *mut i32) -> i32 {
    if a.is_null() || clock.is_null() {
        return EINVAL;
    }
    *clock = *(a as *const i32) & 1;
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
unsafe fn rw_take(p: *mut u8, write: bool, attempt: bool, deadline: *const i64, clock: i32) -> i32 {
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
    (*r).gate.lock();
    // Linux's policy: a lock or timed lock by the thread holding the rwlock
    // for writing would wait for itself, so it fails with EDEADLK (a try
    // reports EBUSY below).
    if !attempt && (*r).writer != 0 && (*r).writer == (*thread).id.load(Relaxed) {
        (*r).gate.unlock();
        return EDEADLK;
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
                (*r).writer = (*thread).id.load(Relaxed);
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
        let result = deadline_wait(&(*r).sequence, seq, deadline, clock);
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
        return ENOTSUP;
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
    rw_take(p, false, false, null(), 0)
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_wrlock(p: *mut u8) -> i32 {
    rw_take(p, true, false, null(), 0)
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_tryrdlock(p: *mut u8) -> i32 {
    rw_take(p, false, true, null(), 0)
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_trywrlock(p: *mut u8) -> i32 {
    rw_take(p, true, true, null(), 0)
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_timedrdlock(p: *mut u8, t: *const i64) -> i32 {
    if t.is_null() {
        EINVAL
    } else {
        rw_take(p, false, false, t, 0)
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_clockrdlock(p: *mut u8, clock: i32, t: *const i64) -> i32 {
    if t.is_null() || !(0..=1).contains(&clock) {
        EINVAL
    } else {
        rw_take(p, false, false, t, clock)
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_timedwrlock(p: *mut u8, t: *const i64) -> i32 {
    if t.is_null() {
        EINVAL
    } else {
        rw_take(p, true, false, t, 0)
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_clockwrlock(p: *mut u8, clock: i32, t: *const i64) -> i32 {
    if t.is_null() || !(0..=1).contains(&clock) {
        EINVAL
    } else {
        rw_take(p, true, false, t, clock)
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
        if (*r).writer != current_id() {
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
    leaving: AtomicU32,
}
#[no_mangle]
pub unsafe extern "C" fn pthread_barrier_init(p: *mut u8, attr: *const u8, count: u32) -> i32 {
    if p.is_null() || count == 0 {
        return EINVAL;
    }
    if !attr.is_null() && !(0..=1).contains(&*(attr as *const i32)) {
        return EINVAL;
    }
    core::ptr::write(
        p as *mut Barrier,
        Barrier {
            gate: Lock::new(),
            count,
            arrived: 0,
            generation: AtomicU32::new(0),
            leaving: AtomicU32::new(0),
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
    (*b).leaving.fetch_add(1, Relaxed);
    if (*b).arrived == (*b).count {
        (*b).arrived = 0;
        (*b).generation.fetch_add(1, Release);
        (*b).gate.unlock();
        wake(&(*b).generation, u32::MAX);
        depart(core::ptr::addr_of!((*b).leaving));
        return -1;
    }
    (*b).gate.unlock();
    while (*b).generation.load(Acquire) == g {
        wait(&(*b).generation, g);
    }
    depart(core::ptr::addr_of!((*b).leaving));
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
        drain(&b.leaving);
        0
    }
}
unsafe extern "C" fn cancel_once(arg: *mut u8) {
    let w = word(arg as *const u32);
    w.store(0, Release);
    wake(w, u32::MAX);
}
#[no_mangle]
pub unsafe extern "C" fn pthread_once(p: *mut i32, init: extern "C" fn()) -> i32 {
    if p.is_null() {
        return EINVAL;
    }
    let w = word(p as *const u32);
    loop {
        if w.compare_exchange(0, 1, Acquire, Acquire).is_ok() {
            let mut cleanup = cancel::Cleanup::new();
            cancel::__breenix_cleanup_push(&mut cleanup, cancel_once, p as *mut u8);
            init();
            cancel::__breenix_cleanup_pop(&mut cleanup, 0);
            w.store(2, Release);
            wake(w, u32::MAX);
            break;
        }
        if w.load(Acquire) == 2 { break; }
        wait(w, 1);
    }
    0
}

pub fn signal_current(sig: i32) -> i64 {
    unsafe { raw::syscall3(nr::TGKILL, pid() as u64, tid() as u64, sig as u64) as i64 }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_kill(handle: usize, sig: i32) -> i32 {
    // The cancel signal is reserved to this library (signal.h).
    if !(0..=64).contains(&sig) || sig == crate::signal::CANCEL_SIGNAL {
        return EINVAL;
    }
    // POSIX requires a handle whose lifetime has not ended. Such a handle
    // pins its descriptor until join (or detached termination); traversing
    // the reclaiming registry would make this signal-safe API deadlock.
    let p = handle as *const Thread;
    if p.is_null() {
        return ESRCH;
    }
    let id = (*p).id.load(Relaxed);
    let r = raw::syscall3(nr::TGKILL, pid() as u64, id as u64, sig as u64) as i64;
    if r < 0 {
        -r as i32
    } else {
        0
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_sigmask(how: i32, set: *const u64, old: *mut u64) -> i32 {
    // An application set never names the reserved cancel signal; a new mask
    // keeps it blocked while this thread has cancellation disabled.
    let public_set = if set.is_null() {
        0
    } else if how == 2 {
        (*set & !crate::signal::CANCEL_MASK) | reserved_block()
    } else {
        *set & !crate::signal::CANCEL_MASK
    };
    let input = if set.is_null() { null() } else { &public_set as *const u64 };
    let r = raw::syscall4(nr::SIGPROCMASK, how as u64, input as u64, old as u64, 8) as i64;
    if r == 0 && !old.is_null() { *old &= !crate::signal::CANCEL_MASK; }
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
        let mask = *set & !crate::signal::CANCEL_MASK;
        let r = cancel_syscall(nr::SIGTIMEDWAIT, [&mask as *const u64 as u64, 0, 0, 8, 0, 0]);
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
    REGISTRY.lock();
    let p = find(handle);
    if p.is_null() {
        REGISTRY.unlock();
        return ESRCH;
    }
    let r = raw::syscall3(
        nr::SCHED_SETSCHEDULER,
        (*p).id.load(Relaxed) as u64,
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
    REGISTRY.lock();
    let p = find(handle);
    if p.is_null() {
        REGISTRY.unlock();
        return ESRCH;
    }
    let r = raw::syscall1(nr::SCHED_GETSCHEDULER, (*p).id.load(Relaxed) as u64) as i64;
    if r < 0 {
        REGISTRY.unlock();
        return -r as i32;
    }
    *policy = r as i32;
    let r = raw::syscall2(nr::SCHED_GETPARAM, (*p).id.load(Relaxed) as u64, param as u64) as i64;
    REGISTRY.unlock();
    if r < 0 {
        -r as i32
    } else {
        0
    }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_setschedprio(handle: usize, priority: i32) -> i32 {
    REGISTRY.lock();
    let p = find(handle);
    if p.is_null() {
        REGISTRY.unlock();
        return ESRCH;
    }
    let r = raw::syscall2(
        nr::SCHED_SETPARAM,
        (*p).id.load(Relaxed) as u64,
        &priority as *const i32 as u64,
    ) as i64;
    REGISTRY.unlock();
    if r < 0 {
        -r as i32
    } else {
        0
    }
}

// Attribute process sharing is encoded independently of each object's other
// attributes. rwlocks retain per-thread read ownership, so shared rwlocks
// explicitly report ENOTSUP rather than claiming to support that mode. A
// robust mutex recovers only when its owner thread ends through pthread_exit
// in this process; no kernel robust list reports an owner in another process
// dying, so a mutex is never both robust and process-shared (ENOTSUP, which
// POSIX.1-2024 2.3 lets an implementation add).
macro_rules! sharing_attr {
    ($set:ident, $get:ident, $mask:expr, $supported:expr) => {
        #[no_mangle]
        pub unsafe extern "C" fn $set(a: *mut u8, shared: i32) -> i32 {
            if a.is_null() || !(0..=1).contains(&shared) { return EINVAL; }
            if shared == 1 && !$supported(*(a as *const u32)) { return ENOTSUP; }
            let w = a as *mut u32;
            *w = (*w & !$mask) | if shared == 1 { $mask } else { 0 };
            0
        }
        #[no_mangle]
        pub unsafe extern "C" fn $get(a: *const u8, shared: *mut i32) -> i32 {
            if a.is_null() || shared.is_null() { return EINVAL; }
            *shared = ((*(a as *const u32) & $mask) != 0) as i32;
            0
        }
    };
}
sharing_attr!(pthread_mutexattr_setpshared, pthread_mutexattr_getpshared, MUTEX_SHARED,
    |attr: u32| attr & ROBUST == 0);
sharing_attr!(pthread_condattr_setpshared, pthread_condattr_getpshared, 2, |_| true);
sharing_attr!(pthread_rwlockattr_setpshared, pthread_rwlockattr_getpshared, 1, |_| false);
sharing_attr!(pthread_barrierattr_setpshared, pthread_barrierattr_getpshared, 1, |_| true);
macro_rules! plain_attr {
    ($init:ident, $destroy:ident) => {
        #[no_mangle]
        pub unsafe extern "C" fn $init(a: *mut u8) -> i32 {
            if a.is_null() { return EINVAL; }
            *(a as *mut u32) = 0;
            0
        }
        #[no_mangle]
        pub extern "C" fn $destroy(a: *mut u8) -> i32 { if a.is_null() { EINVAL } else { 0 } }
    };
}
plain_attr!(pthread_rwlockattr_init, pthread_rwlockattr_destroy);
plain_attr!(pthread_barrierattr_init, pthread_barrierattr_destroy);
#[no_mangle]
pub unsafe extern "C" fn pthread_attr_setscope(a: *mut u8, scope: i32) -> i32 {
    if a.is_null() || !(0..=1).contains(&scope) { EINVAL }
    else if scope == 1 { ENOTSUP } else { 0 }
}
#[no_mangle]
pub unsafe extern "C" fn pthread_attr_getscope(a: *const u8, scope: *mut i32) -> i32 {
    if a.is_null() || scope.is_null() { return EINVAL; }
    *scope = 0;
    0
}
