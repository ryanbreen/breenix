//! Memory: anonymous mappings and munmap, mprotect and the faults it produces, shared and
//! private mappings across fork, the program break, and memory locking, msync and
//! advice, as POSIX specifies them.
//!
//! Each case runs in its own forked child under the runner's default 10-second limit.
//! Every wait inside a case is bounded and stops early enough to leave the case time to
//! report. The processes a case starts are killed when it ends, and the runner, which a
//! suite runs as PID 1, kills and reaps any that remain before the next case, so no case
//! depends on another.
//!
//! Cases call the kernel by its Linux numbers and assert on the raw return, so an
//! unimplemented call fails with ENOSYS. On x86-64 they use the SYSCALL instruction, as a
//! C library does. Library-level interfaces are made as a C library makes them: sbrk
//! from brk, posix_madvise through madvise, and setrlimit through prlimit64.
//!
//! Faults are taken in the case's own process. A SA_SIGINFO handler records the signal,
//! si_code and si_addr of each SIGSEGV, SIGBUS or SIGILL, and resumes only from the three
//! probes below: a one-byte load, a one-byte store and a call, each at a known address,
//! so a fault anywhere else still kills the case. Children report back through pipes,
//! never through the memory under test.
use libbreenix::process::{self, ForkResult};
use libbreenix::signal::Sigaction;
use libbreenix::suite::{case, case_ms_left, category, check, fail, suite, value, CaseError, CaseResult, Suite};
#[cfg(target_arch = "aarch64")]
use libbreenix::syscall::raw;
use std::ffi::CString;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering::SeqCst};

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const READ: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const CLOSE: u64 = 3;
    pub const LSEEK: u64 = 8;
    pub const MMAP: u64 = 9;
    pub const MPROTECT: u64 = 10;
    pub const MUNMAP: u64 = 11;
    pub const BRK: u64 = 12;
    pub const RT_SIGACTION: u64 = 13;
    pub const PREAD64: u64 = 17;
    pub const PWRITE64: u64 = 18;
    pub const MSYNC: u64 = 26;
    pub const MINCORE: u64 = 27;
    pub const MADVISE: u64 = 28;
    pub const NANOSLEEP: u64 = 35;
    pub const GETPID: u64 = 39;
    pub const WAIT4: u64 = 61;
    pub const KILL: u64 = 62;
    pub const FTRUNCATE: u64 = 77;
    pub const SETUID: u64 = 105;
    pub const MLOCK: u64 = 149;
    pub const MUNLOCK: u64 = 150;
    pub const MLOCKALL: u64 = 151;
    pub const MUNLOCKALL: u64 = 152;
    pub const CLOCK_GETTIME: u64 = 228;
    pub const OPENAT: u64 = 257;
    pub const UNLINKAT: u64 = 263;
    pub const PIPE2: u64 = 293;
    pub const PRLIMIT64: u64 = 302;
}
#[cfg(target_arch = "aarch64")]
mod nr {
    pub const READ: u64 = 63;
    pub const WRITE: u64 = 64;
    pub const CLOSE: u64 = 57;
    pub const LSEEK: u64 = 62;
    pub const MMAP: u64 = 222;
    pub const MPROTECT: u64 = 226;
    pub const MUNMAP: u64 = 215;
    pub const BRK: u64 = 214;
    pub const RT_SIGACTION: u64 = 134;
    pub const PREAD64: u64 = 67;
    pub const PWRITE64: u64 = 68;
    pub const MSYNC: u64 = 227;
    pub const MINCORE: u64 = 232;
    pub const MADVISE: u64 = 233;
    pub const NANOSLEEP: u64 = 101;
    pub const GETPID: u64 = 172;
    pub const WAIT4: u64 = 260;
    pub const KILL: u64 = 129;
    pub const FTRUNCATE: u64 = 46;
    pub const SETUID: u64 = 146;
    pub const MLOCK: u64 = 228;
    pub const MUNLOCK: u64 = 229;
    pub const MLOCKALL: u64 = 230;
    pub const MUNLOCKALL: u64 = 231;
    pub const CLOCK_GETTIME: u64 = 113;
    pub const OPENAT: u64 = 56;
    pub const UNLINKAT: u64 = 35;
    pub const PIPE2: u64 = 59;
    pub const PRLIMIT64: u64 = 261;
}

const EPERM: i64 = 1;
const EBADF: i64 = 9;
const EAGAIN: i64 = 11;
const ENOMEM: i64 = 12;
const EACCES: i64 = 13;
const EBUSY: i64 = 16;
const EEXIST: i64 = 17;
const ENODEV: i64 = 19;
const EINVAL: i64 = 22;

const SIGILL: i32 = 4;
const SIGBUS: i32 = 7;
const SIGKILL: i32 = 9;
const SIGSEGV: i32 = 11;
const SEGV_MAPERR: i32 = 1;
const SEGV_ACCERR: i32 = 2;

const SIG_DFL: u64 = 0;
const SA_SIGINFO: u64 = 4;
const SA_RESTORER: u64 = 0x0400_0000;

const PAGE: usize = 4096;
const MIB: usize = 1 << 20;

const PROT_NONE: i32 = 0;
const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const PROT_EXEC: i32 = 4;
const RW: i32 = PROT_READ | PROT_WRITE;

const MAP_SHARED: i32 = 0x01;
const MAP_PRIVATE: i32 = 0x02;
const MAP_FIXED: i32 = 0x10;
const MAP_ANONYMOUS: i32 = 0x20;
const MAP_FIXED_NOREPLACE: i32 = 0x10_0000;
const PRIVATE: i32 = MAP_PRIVATE | MAP_ANONYMOUS;
const SHARED: i32 = MAP_SHARED | MAP_ANONYMOUS;

const MS_ASYNC: u64 = 1;
const MS_INVALIDATE: u64 = 2;
const MS_SYNC: u64 = 4;

const MCL_CURRENT: u64 = 1;
const MCL_FUTURE: u64 = 2;

const POSIX_MADV_NORMAL: u64 = 0;
const POSIX_MADV_RANDOM: u64 = 1;
const POSIX_MADV_SEQUENTIAL: u64 = 2;
const POSIX_MADV_WILLNEED: u64 = 3;
/// Linux's MADV_DONTNEED, which discards private anonymous pages.
const MADV_DONTNEED: u64 = 4;

const RLIMIT_DATA: u32 = 2;
const RLIMIT_MEMLOCK: u32 = 8;
const RLIMIT_AS: u32 = 9;
const RLIM_INFINITY: u64 = u64::MAX;

const O_RDONLY: u64 = 0;
const O_WRONLY: u64 = 1;
const O_RDWR: u64 = 2;
const O_CREAT: u64 = 0x40;
const O_TRUNC: u64 = 0x200;
const O_NONBLOCK: u64 = 0x800;
const O_CLOEXEC: u64 = 0x8_0000;
const AT_FDCWD: i64 = -100;
const SEEK_END: u64 = 2;

const WNOHANG: i32 = 1;

const NS: i64 = 1_000_000_000;
const MS: i64 = 1_000_000;

/// A user of the suite's own, for the cases that need an unprivileged process.
const USER_A: u32 = 4242;

/// How long a case waits for another process. A working kernel takes milliseconds; the
/// bound only keeps a failing case inside the runner's limit.
const WAIT_MS: u64 = 3000;
/// What a case's waits leave of the runner's limit, for the case to clean up and report.
const CLEANUP_MS: u64 = 1500;

/// A function that returns 42: `mov w0, #42; ret` on ARM64, `mov eax, 42; ret` on x86-64.
#[cfg(target_arch = "aarch64")]
const CODE42: [u8; 8] = [0x40, 0x05, 0x80, 0x52, 0xc0, 0x03, 0x5f, 0xd6];
#[cfg(target_arch = "x86_64")]
const CODE42: [u8; 6] = [0xb8, 0x2a, 0x00, 0x00, 0x00, 0xc3];

/// A system call made as a C library makes it: on x86-64 the SYSCALL instruction, so
/// the cases run the kernel's SYSCALL entry and return path, and on ARM64 svc.
fn sc(n: u64, args: &[u64]) -> i64 {
    let a = |i: usize| args.get(i).copied().unwrap_or(0);
    #[cfg(target_arch = "x86_64")]
    {
        let ret: i64;
        // SAFETY: every caller keeps the buffers its arguments point to alive through the
        // call; SYSCALL writes only rax, rcx and r11.
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") n as i64 => ret,
                in("rdi") a(0), in("rsi") a(1), in("rdx") a(2), in("r10") a(3), in("r8") a(4), in("r9") a(5),
                lateout("rcx") _, lateout("r11") _,
                options(nostack),
            );
        }
        ret
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: every caller keeps the buffers its arguments point to alive through the call.
    unsafe { raw::syscall6(n, a(0), a(1), a(2), a(3), a(4), a(5)) as i64 }
}

/// The signal restorer the suite's handlers return through: rt_sigreturn made the way
/// `sc` makes calls.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
extern "C" fn restore_rt() -> ! {
    core::arch::naked_asm!("mov rax, 15", "syscall", "ud2")
}
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
extern "C" fn restore_rt() -> ! {
    core::arch::naked_asm!("mov x8, 139", "svc #0", "brk #1")
}

fn errname(errno: i64) -> String {
    let name = match errno {
        1 => "EPERM", 2 => "ENOENT", 3 => "ESRCH", 4 => "EINTR", 9 => "EBADF", 10 => "ECHILD",
        11 => "EAGAIN", 12 => "ENOMEM", 13 => "EACCES", 14 => "EFAULT", 16 => "EBUSY", 17 => "EEXIST",
        19 => "ENODEV", 22 => "EINVAL", 38 => "ENOSYS", 95 => "EOPNOTSUPP",
        _ => return format!("errno {errno}"),
    };
    name.to_string()
}

/// Whether a raw return is an error: -4095 to -1, as a C library tells them apart.
fn is_err(ret: i64) -> bool { (-4095..0).contains(&ret) }

/// A raw return as text: the errno name for an error, else the value.
fn shown(ret: i64) -> String { if is_err(ret) { errname(-ret) } else if ret > 0xffff { format!("{ret:#x}") } else { ret.to_string() } }

fn err<T>(msg: impl Into<String>) -> Result<T, CaseError> { Err(CaseError::Fail(msg.into())) }

/// A call that returns 0 when it succeeds: any other return fails, naming the call.
fn zero(what: &str, ret: i64) -> CaseResult {
    match ret {
        0 => Ok(()),
        r if is_err(r) => fail(format!("{what} failed with {}", errname(-r))),
        r => fail(format!("{what} returned {}, not 0", shown(r))),
    }
}

fn want_err(what: &str, got: i64, errno: i64) -> CaseResult {
    check(got == -errno, &format!("{what}: expected {}, got {}", errname(errno), shown(got)))
}

fn pid() -> i32 { sc(nr::GETPID, &[]) as i32 }
fn kill(pid: i32, sig: i32) -> i64 { sc(nr::KILL, &[pid as i64 as u64, sig as i64 as u64]) }
fn wait4(pid: i32, status: *mut i32, options: i32) -> i64 {
    sc(nr::WAIT4, &[pid as i64 as u64, status as u64, options as u64, 0])
}

// ---------------------------------------------------------------------------
// Time.

fn mono() -> i64 {
    let mut t = [0i64; 2];
    if sc(nr::CLOCK_GETTIME, &[1, t.as_mut_ptr() as u64]) != 0 { return 0; }
    t[0] * NS + t[1]
}
fn now_ms() -> u64 { (mono() / MS) as u64 }

/// `ms`, cut short so that `reserve` ms of the case's limit remain afterwards.
fn bounded(ms: u64, reserve: u64) -> u64 { ms.min(case_ms_left().saturating_sub(reserve)) }

fn nap() {
    let t = [0i64, MS];
    let _ = sc(nr::NANOSLEEP, &[t.as_ptr() as u64, 0]);
}

/// Wait up to `ms` for `cond`, napping between looks.
fn until(ms: u64, mut cond: impl FnMut() -> bool) -> bool {
    let ms = bounded(ms, CLEANUP_MS);
    let start = now_ms();
    loop {
        if cond() { return true; }
        if now_ms().saturating_sub(start) >= ms { return false; }
        nap();
    }
}

/// Spin up to `ms` for `cond`, without sleeping, for the cases timed across processors.
fn spin_until(ms: u64, mut cond: impl FnMut() -> bool) -> bool {
    let start = now_ms();
    let mut spins = 0u64;
    loop {
        if cond() { return true; }
        spins += 1;
        if spins % 1024 == 0 && now_ms().saturating_sub(start) >= ms { return false; }
        core::hint::spin_loop();
    }
}

// ---------------------------------------------------------------------------
// Mappings.

fn mmap_raw(addr: u64, len: u64, prot: i32, flags: i32, fd: i32, off: u64) -> i64 {
    sc(nr::MMAP, &[addr, len, prot as u64, flags as u32 as u64, fd as i64 as u64, off])
}
fn munmap(p: *mut u8, len: usize) -> i64 { sc(nr::MUNMAP, &[p as u64, len as u64]) }
fn mprotect(p: *mut u8, len: usize, prot: i32) -> i64 { sc(nr::MPROTECT, &[p as u64, len as u64, prot as u64]) }
fn msync(p: *mut u8, len: usize, flags: u64) -> i64 { sc(nr::MSYNC, &[p as u64, len as u64, flags]) }
fn madvise(p: *mut u8, len: usize, advice: u64) -> i64 { sc(nr::MADVISE, &[p as u64, len as u64, advice]) }
fn mlock(p: *mut u8, len: usize) -> i64 { sc(nr::MLOCK, &[p as u64, len as u64]) }
fn munlock(p: *mut u8, len: usize) -> i64 { sc(nr::MUNLOCK, &[p as u64, len as u64]) }
fn mlockall(flags: u64) -> i64 { sc(nr::MLOCKALL, &[flags]) }
fn munlockall() -> i64 { sc(nr::MUNLOCKALL, &[]) }
fn mincore(p: *mut u8, len: usize, vec: &mut [u8]) -> i64 { sc(nr::MINCORE, &[p as u64, len as u64, vec.as_mut_ptr() as u64]) }

fn prot_name(prot: i32) -> &'static str {
    match prot {
        0 => "PROT_NONE", 1 => "PROT_READ", 3 => "PROT_READ|PROT_WRITE", 5 => "PROT_READ|PROT_EXEC",
        _ => "prot",
    }
}

/// A new mapping, or a failure naming it.
fn map(addr: u64, len: usize, prot: i32, flags: i32, fd: i32) -> Result<*mut u8, CaseError> {
    let r = mmap_raw(addr, len as u64, prot, flags, fd, 0);
    if is_err(r) { return err(format!("mmap of {len} bytes ({}) failed with {}", prot_name(prot), errname(-r))); }
    Ok(r as u64 as *mut u8)
}

/// A private anonymous read-write mapping of `len` bytes.
fn anon(len: usize) -> Result<*mut u8, CaseError> { map(0, len, RW, PRIVATE, -1) }

fn unmap(p: *mut u8, len: usize) -> CaseResult { zero("munmap", munmap(p, len)) }
fn protect(p: *mut u8, len: usize, prot: i32) -> CaseResult { zero(&format!("mprotect({})", prot_name(prot)), mprotect(p, len, prot)) }

/// A page-aligned range of `len` bytes with nothing mapped in it, found by mapping and
/// unmapping it. Nothing may map between this and the caller's use of it.
fn free_range(len: usize) -> Result<u64, CaseError> {
    let p = map(0, len, PROT_NONE, PRIVATE, -1)?;
    unmap(p, len)?;
    Ok(p as u64)
}

fn at(p: *mut u8, off: usize) -> *mut u8 { p.wrapping_add(off) }
fn peek(p: *mut u8) -> u8 {
    // SAFETY: callers read only memory they mapped readable.
    unsafe { core::ptr::read_volatile(p) }
}
fn poke(p: *mut u8, v: u8) {
    // SAFETY: callers write only memory they mapped writable.
    unsafe { core::ptr::write_volatile(p, v) }
}

/// The byte a pattern puts at offset `off`.
fn pat(off: usize, seed: u8) -> u8 { ((off / PAGE) as u8).wrapping_mul(31) ^ (off % 251) as u8 ^ seed }

fn fill(p: *mut u8, len: usize, seed: u8) { for off in 0..len { poke(at(p, off), pat(off, seed)); } }
fn fill_byte(p: *mut u8, len: usize, v: u8) { for off in 0..len { poke(at(p, off), v); } }

/// The first offset in `len` bytes at `p` that does not hold the pattern, if any.
fn pattern_gap(p: *mut u8, len: usize, seed: u8) -> Option<usize> { (0..len).find(|&off| peek(at(p, off)) != pat(off, seed)) }
/// The first offset in `len` bytes at `p` that does not hold `v`, if any.
fn byte_gap(p: *mut u8, len: usize, v: u8) -> Option<usize> { (0..len).find(|&off| peek(at(p, off)) != v) }

fn want_pattern(p: *mut u8, len: usize, seed: u8, what: &str) -> CaseResult {
    match pattern_gap(p, len, seed) {
        None => Ok(()),
        Some(off) => fail(format!("{what}: byte {off} reads {:#04x}, not {:#04x}", peek(at(p, off)), pat(off, seed))),
    }
}
fn want_bytes(p: *mut u8, len: usize, v: u8, what: &str) -> CaseResult {
    match byte_gap(p, len, v) {
        None => Ok(()),
        Some(off) => fail(format!("{what}: byte {off} reads {:#04x}, not {v:#04x}", peek(at(p, off)))),
    }
}

/// A count from /proc/<pid>/status, in kB.
fn status_kb(field: &str) -> Result<i64, CaseError> {
    let text = std::fs::read_to_string(format!("/proc/{}/status", pid())).map_err(|e| format!("reading /proc/<pid>/status failed: {e}"))?;
    text.lines().find_map(|line| line.strip_prefix(field))
        .and_then(|rest| rest.trim().split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| CaseError::Fail(format!("/proc/<pid>/status has no {field} line")))
}

/// The process's resident pages, from VmRSS in /proc/<pid>/status.
fn resident() -> Result<i64, CaseError> { Ok(status_kb("VmRSS:")? / 4) }

/// Free memory in kB, from MemFree in /proc/meminfo.
fn mem_free_kb() -> Result<i64, CaseError> {
    let text = std::fs::read_to_string("/proc/meminfo").map_err(|e| format!("reading /proc/meminfo failed: {e}"))?;
    text.lines().find_map(|line| line.strip_prefix("MemFree:"))
        .and_then(|rest| rest.trim().split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| CaseError::Fail("/proc/meminfo has no MemFree line".into()))
}

/// Processors online, from the `processor` lines of /proc/cpuinfo. A census that cannot
/// be read or counts none fails the case rather than passing for one processor.
fn processors() -> Result<usize, CaseError> {
    let info = std::fs::read_to_string("/proc/cpuinfo").map_err(|e| format!("reading /proc/cpuinfo failed: {e}"))?;
    let n = info.lines().filter(|line| line.starts_with("processor")).count();
    if n == 0 { return err("/proc/cpuinfo lists no processor"); }
    Ok(n)
}

/// setrlimit as a C library makes it, through prlimit64 on this process.
fn set_limit(resource: u32, soft: u64, hard: u64) -> i64 {
    let new = [soft, hard];
    sc(nr::PRLIMIT64, &[0, resource as u64, new.as_ptr() as u64, 0])
}

// ---------------------------------------------------------------------------
// Faults.

// The three probes the fault handler resumes from, and the cache maintenance a JIT does.
// Each faulting instruction is the first of its function, and each `_resume` label is
// where the handler sends a probe that faulted, with the return register set to say so.
#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".text",
    ".balign 4",
    ".globl mem_probe_load", ".globl mem_probe_load_resume",
    ".globl mem_probe_store", ".globl mem_probe_store_resume",
    ".globl mem_probe_call", ".globl mem_probe_call_return",
    ".globl mem_clear_cache", ".globl mem_clear_cache_resume",
    "mem_probe_load:",
    "ldrb w0, [x0]",
    "mem_probe_load_resume:",
    "ret",
    "mem_probe_store:",
    "strb w1, [x0]",
    "mov w0, #0",
    "mem_probe_store_resume:",
    "ret",
    "mem_probe_call:",
    "stp x29, x30, [sp, #-16]!",
    "mov x29, sp",
    "blr x0",
    "mem_probe_call_return:",
    "ldp x29, x30, [sp], #16",
    "ret",
    "mem_clear_cache:",
    "dc cvau, x0",
    "dsb ish",
    "ic ivau, x0",
    "dsb ish",
    "isb",
    "mov x0, #0",
    "mem_clear_cache_resume:",
    "ret",
);
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".text",
    ".globl mem_probe_load", ".globl mem_probe_load_resume",
    ".globl mem_probe_store", ".globl mem_probe_store_resume",
    ".globl mem_probe_call", ".globl mem_probe_call_return",
    ".globl mem_clear_cache", ".globl mem_clear_cache_resume",
    "mem_probe_load:",
    "movzx eax, byte ptr [rdi]",
    "mem_probe_load_resume:",
    "ret",
    "mem_probe_store:",
    "mov byte ptr [rdi], sil",
    "xor eax, eax",
    "mem_probe_store_resume:",
    "ret",
    "mem_probe_call:",
    "sub rsp, 8",
    "call rdi",
    "mem_probe_call_return:",
    "add rsp, 8",
    "ret",
    "mem_clear_cache:",
    "xor eax, eax",
    "mem_clear_cache_resume:",
    "ret",
);

extern "C" {
    fn mem_probe_load(p: *const u8) -> u32;
    fn mem_probe_load_resume();
    fn mem_probe_store(p: *mut u8, v: u32) -> u32;
    fn mem_probe_store_resume();
    fn mem_probe_call(target: *const u8) -> u64;
    fn mem_probe_call_return();
    fn mem_clear_cache(p: *const u8) -> u64;
    fn mem_clear_cache_resume();
}

/// Offsets in the Linux ucontext_t of the saved pc and return register, and on x86-64 the
/// stack pointer.
#[cfg(target_arch = "aarch64")]
mod uc {
    pub const RET: usize = 184;
    pub const PC: usize = 440;
}
#[cfg(target_arch = "x86_64")]
mod uc {
    pub const RET: usize = 40 + 13 * 8;
    pub const SP: usize = 40 + 15 * 8;
    pub const PC: usize = 40 + 16 * 8;
}

/// siginfo_t as the Linux ABI lays it out: signo, errno and code, then si_addr.
#[repr(C)]
struct SigInfo { signo: i32, errno: i32, code: i32, pad: i32, addr: u64 }

static FAULTS: AtomicU64 = AtomicU64::new(0);
static ACCERR: AtomicU64 = AtomicU64::new(0);
static MAPERR: AtomicU64 = AtomicU64::new(0);
static LAST_SIG: AtomicI32 = AtomicI32::new(0);
static LAST_CODE: AtomicI32 = AtomicI32::new(0);
static LAST_ADDR: AtomicU64 = AtomicU64::new(0);
/// While set, the handler makes a faulting page readable and writable and retries.
static REPAIR: AtomicU32 = AtomicU32::new(0);
/// The most faults REPAIR answers, so a repair that does not take cannot loop forever.
const REPAIR_MAX: u64 = 1000;
/// The address `call` jumps to.
static EXEC_TARGET: AtomicU64 = AtomicU64::new(0);

/// The value `mem_probe_load` returns when the load faulted.
const LOAD_FAULTED: u64 = 0x100;

fn sym(f: unsafe extern "C" fn()) -> u64 { f as usize as u64 }

/// SAFETY: `uc` is the ucontext_t the kernel passed this delivery.
unsafe fn uc_get(uc: *mut u8, off: usize) -> u64 { core::ptr::read_volatile(uc.add(off) as *const u64) }
/// SAFETY: as `uc_get`; the word is one the kernel restores at sigreturn.
unsafe fn uc_set(uc: *mut u8, off: usize, v: u64) { core::ptr::write_volatile(uc.add(off) as *mut u64, v) }

extern "C" fn on_fault(sig: i32, info: *const SigInfo, uc: *mut u8) {
    // SAFETY: the kernel passes the siginfo it wrote for this delivery.
    let (code, addr) = if info.is_null() { (-1, 0) } else { unsafe { ((*info).code, (*info).addr) } };
    LAST_SIG.store(sig, SeqCst);
    LAST_CODE.store(code, SeqCst);
    LAST_ADDR.store(addr, SeqCst);
    let n = FAULTS.fetch_add(1, SeqCst) + 1;
    if sig == SIGSEGV && code == SEGV_ACCERR { ACCERR.fetch_add(1, SeqCst); }
    if sig == SIGSEGV && code == SEGV_MAPERR { MAPERR.fetch_add(1, SeqCst); }
    if uc.is_null() { give_up(sig); return; }
    if REPAIR.load(SeqCst) == 1 && sig == SIGSEGV && n <= REPAIR_MAX {
        let page = addr & !(PAGE as u64 - 1);
        if sc(nr::MPROTECT, &[page, PAGE as u64, RW as u64]) == 0 { return; }
    }
    // SAFETY: the kernel passes the ucontext_t it saved for this delivery.
    let pc = unsafe { uc_get(uc, uc::PC) };
    let resume = if pc == mem_probe_load as usize as u64 {
        Some((sym(mem_probe_load_resume), LOAD_FAULTED))
    } else if pc == mem_probe_store as usize as u64 {
        Some((sym(mem_probe_store_resume), 1))
    } else if pc != 0 && pc == EXEC_TARGET.load(SeqCst) {
        // The call pushed its return address on x86-64; drop it, as the return would.
        #[cfg(target_arch = "x86_64")]
        // SAFETY: as above.
        unsafe { uc_set(uc, uc::SP, uc_get(uc, uc::SP) + 8) };
        Some((sym(mem_probe_call_return), u64::MAX))
    } else if sig == SIGILL && pc >= mem_clear_cache as usize as u64 && pc < sym(mem_clear_cache_resume) {
        Some((sym(mem_clear_cache_resume), 1))
    } else {
        None
    };
    match resume {
        // SAFETY: as above.
        Some((to, ret)) => unsafe {
            uc_set(uc, uc::RET, ret);
            uc_set(uc, uc::PC, to);
        },
        None => give_up(sig),
    }
}

/// Restore the default action, so returning re-raises the fault and kills the process.
fn give_up(sig: i32) {
    let act = Sigaction { handler: SIG_DFL, mask: 0, flags: 0, restorer: 0 };
    let _ = sc(nr::RT_SIGACTION, &[sig as u64, &act as *const Sigaction as u64, 0, 8]);
}

/// Take SIGSEGV, SIGBUS and SIGILL with `on_fault`.
fn catch_faults() -> CaseResult {
    for sig in [SIGSEGV, SIGBUS, SIGILL] {
        let act = Sigaction { handler: on_fault as usize as u64, mask: 0, flags: SA_SIGINFO | SA_RESTORER, restorer: restore_rt as usize as u64 };
        zero(&format!("sigaction({sig})"), sc(nr::RT_SIGACTION, &[sig as u64, &act as *const Sigaction as u64, 0, 8]))?;
    }
    Ok(())
}

/// A fault a probe took: the signal, si_code and si_addr.
#[derive(Clone, Copy)]
struct Fault { sig: i32, code: i32, addr: u64 }

fn last_fault() -> Fault { Fault { sig: LAST_SIG.load(SeqCst), code: LAST_CODE.load(SeqCst), addr: LAST_ADDR.load(SeqCst) } }

fn sig_name(sig: i32) -> String {
    match sig { 4 => "SIGILL".into(), 7 => "SIGBUS".into(), 11 => "SIGSEGV".into(), s => format!("signal {s}") }
}
fn code_name(sig: i32, code: i32) -> String {
    match (sig, code) {
        (11, 1) => "SEGV_MAPERR".into(), (11, 2) => "SEGV_ACCERR".into(),
        (7, 1) => "BUS_ADRALN".into(), (7, 2) => "BUS_ADRERR".into(), (7, 3) => "BUS_OBJERR".into(),
        (_, c) => format!("si_code {c}"),
    }
}
fn fault_text(f: Fault) -> String { format!("{} {} at {:#x}", sig_name(f.sig), code_name(f.sig, f.code), f.addr) }

/// Load one byte; the fault if the load faulted.
fn load(p: *const u8) -> Result<u8, Fault> {
    LAST_SIG.store(0, SeqCst);
    // SAFETY: a fault in the probe is answered by `on_fault`, which resumes past it.
    let r = unsafe { mem_probe_load(p) } as u64;
    if r == LOAD_FAULTED { Err(last_fault()) } else { Ok(r as u8) }
}

/// Store one byte; the fault if the store faulted.
fn store(p: *mut u8, v: u8) -> Result<(), Fault> {
    LAST_SIG.store(0, SeqCst);
    // SAFETY: as `load`.
    let r = unsafe { mem_probe_store(p, v as u32) };
    if r != 0 { Err(last_fault()) } else { Ok(()) }
}

/// Call the code at `p`; what it returned, or the fault if the call faulted.
fn call(p: *const u8) -> Result<u64, Fault> {
    LAST_SIG.store(0, SeqCst);
    EXEC_TARGET.store(p as u64, SeqCst);
    // SAFETY: callers place a function returning in the return register at `p`; a fault
    // fetching it is answered by `on_fault`, which returns from the call.
    let r = unsafe { mem_probe_call(p) };
    EXEC_TARGET.store(0, SeqCst);
    if r == u64::MAX { Err(last_fault()) } else { Ok(r) }
}

/// The access at `p` must fault with `sig`, `code` (when given) and si_addr `p`.
fn want_fault(what: &str, got: Result<u8, Fault>, sig: i32, code: Option<i32>, p: *const u8) -> CaseResult {
    match got {
        Ok(v) => fail(format!("{what} succeeded (read {v:#04x}) instead of raising {}", sig_name(sig))),
        Err(f) => {
            check(f.sig == sig, &format!("{what} raised {}, not {}", fault_text(f), sig_name(sig)))?;
            if let Some(code) = code {
                check(f.code == code, &format!("{what} raised {}, not {}", fault_text(f), code_name(sig, code)))?;
            }
            check(f.addr == p as u64, &format!("{what} raised {} with si_addr {:#x}, not the address touched {:#x}", sig_name(sig), f.addr, p as u64))
        }
    }
}
fn load_faults(what: &str, p: *mut u8, sig: i32, code: Option<i32>) -> CaseResult { want_fault(what, load(p), sig, code, p) }
fn store_faults(what: &str, p: *mut u8, code: i32) -> CaseResult { want_fault(what, store(p, 0xee).map(|_| 0), SIGSEGV, Some(code), p) }

/// The byte at `p` must load without a fault, and be `v` when given.
fn loads(what: &str, p: *mut u8, v: Option<u8>) -> CaseResult {
    match load(p) {
        Err(f) => fail(format!("{what} raised {}", fault_text(f))),
        Ok(got) => match v {
            Some(v) if got != v => fail(format!("{what} read {got:#04x}, not {v:#04x}")),
            _ => Ok(()),
        },
    }
}
fn stores(what: &str, p: *mut u8, v: u8) -> CaseResult {
    store(p, v).map_err(|f| CaseError::Fail(format!("{what} raised {}", fault_text(f))))
}

/// Call the code at `p`, which must return 42.
fn runs(what: &str, p: *mut u8) -> CaseResult {
    match call(p) {
        Ok(42) => Ok(()),
        Ok(r) => fail(format!("{what} returned {r}, not 42")),
        Err(f) => fail(format!("{what} raised {}", fault_text(f))),
    }
}
/// Calling the code at `p` must raise SIGSEGV with SEGV_ACCERR at `p`.
fn call_faults(what: &str, p: *mut u8) -> CaseResult {
    match call(p) {
        Ok(r) => fail(format!("{what} ran the code (it returned {r}) instead of raising SIGSEGV")),
        Err(f) => want_fault(what, Err(f), SIGSEGV, Some(SEGV_ACCERR), p),
    }
}

// ---------------------------------------------------------------------------
// Files.

/// A file of the case's own under /tmp; dropping it closes and removes it.
struct TempFile { path: CString, fd: i32 }

impl TempFile {
    fn new(tag: &str, bytes: &[u8]) -> Result<TempFile, CaseError> {
        let path = CString::new(format!("/tmp/memory-{}-{tag}", pid())).expect("path without NUL");
        let fd = open_raw(&path, O_RDWR | O_CREAT | O_TRUNC | O_CLOEXEC)?;
        let file = TempFile { path, fd };
        pwrite_all(fd, bytes, 0)?;
        Ok(file)
    }

    /// Another descriptor for the file, opened with `flags`.
    fn open(&self, flags: u64) -> Result<i32, CaseError> { open_raw(&self.path, flags | O_CLOEXEC) }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = sc(nr::CLOSE, &[self.fd as u64]);
        let _ = sc(nr::UNLINKAT, &[AT_FDCWD as u64, self.path.as_ptr() as u64, 0]);
    }
}

fn open_raw(path: &CString, flags: u64) -> Result<i32, CaseError> {
    let fd = sc(nr::OPENAT, &[AT_FDCWD as u64, path.as_ptr() as u64, flags, 0o644]);
    if is_err(fd) { return err(format!("open {:?} failed with {}", path, errname(-fd))); }
    Ok(fd as i32)
}

fn close(fd: i32) { let _ = sc(nr::CLOSE, &[fd as u64]); }

fn pwrite_all(fd: i32, bytes: &[u8], off: u64) -> CaseResult {
    let r = sc(nr::PWRITE64, &[fd as u64, bytes.as_ptr() as u64, bytes.len() as u64, off]);
    check(r == bytes.len() as i64, &format!("pwrite of {} bytes returned {}", bytes.len(), shown(r)))
}

fn pread_n(fd: i32, len: usize, off: u64) -> Result<Vec<u8>, CaseError> {
    let mut buf = vec![0u8; len];
    let r = sc(nr::PREAD64, &[fd as u64, buf.as_mut_ptr() as u64, len as u64, off]);
    if is_err(r) { return err(format!("pread failed with {}", errname(-r))); }
    buf.truncate(r as usize);
    Ok(buf)
}

/// A page holding a function that returns 42, then zeros.
fn code_page() -> Vec<u8> {
    let mut page = vec![0u8; PAGE];
    page[..CODE42.len()].copy_from_slice(&CODE42);
    page
}

// ---------------------------------------------------------------------------
// Processes.

fn pipe() -> Result<(i32, i32), CaseError> {
    let mut fds = [0i32; 2];
    zero("pipe2", sc(nr::PIPE2, &[fds.as_mut_ptr() as u64, O_NONBLOCK]))?;
    Ok((fds[0], fds[1]))
}

fn send(fd: i32, b: u8) { let buf = [b]; let _ = sc(nr::WRITE, &[fd as u64, buf.as_ptr() as u64, 1]); }

/// One byte from the non-blocking pipe `fd`, waiting up to `ms`; None at EOF or timeout.
fn recv(fd: i32, ms: u64) -> Option<u8> {
    let mut buf = [0u8];
    let mut got = None;
    until(ms, || {
        let r = sc(nr::READ, &[fd as u64, buf.as_mut_ptr() as u64, 1]);
        if r == 1 { got = Some(buf[0]); }
        r != -EAGAIN
    });
    got
}

// Wait statuses, decoded as a C library's macros decode them.
fn exited(s: i32) -> bool { s & 0x7f == 0 }
fn exit_code(s: i32) -> i32 { (s >> 8) & 0xff }

/// A child process. Dropping it while it may still run kills and reaps it.
struct Child { pid: i32, live: bool }

impl Child {
    /// Fork; the child runs `f` and exits with what it returns.
    fn start(f: impl FnOnce() -> i32) -> Result<Child, CaseError> {
        match process::fork()? {
            ForkResult::Child => process::exit(f()),
            ForkResult::Parent(pid) => Ok(Child { pid: pid.raw() as i32, live: true }),
        }
    }

    /// Wait for the child to end. Exit 0 passes; exit k fails with `why[k - 1]`.
    fn finish(&mut self, what: &str, why: &[&str]) -> CaseResult {
        let ms = bounded(WAIT_MS, CLEANUP_MS / 2);
        let start = now_ms();
        let status = loop {
            let mut status = 0;
            let r = wait4(self.pid, &mut status, WNOHANG);
            if r > 0 { break status; }
            if r < 0 { return fail(format!("waitpid({}) failed with {}", self.pid, errname(-r))); }
            if now_ms().saturating_sub(start) >= ms { return fail(format!("{what} did not end within {ms} ms")); }
            nap();
        };
        self.live = false;
        if exited(status) {
            match exit_code(status) {
                0 => Ok(()),
                k => fail(why.get(k as usize - 1).map_or_else(|| format!("{what} exited with {k}"), |w| w.to_string())),
            }
        } else {
            fail(format!("{what} was killed by {}", sig_name(status & 0x7f)))
        }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.live {
            kill(self.pid, SIGKILL);
            let mut status = 0;
            let start = now_ms();
            while wait4(self.pid, &mut status, WNOHANG) == 0 && now_ms().saturating_sub(start) < 500 { nap(); }
        }
    }
}

// ---------------------------------------------------------------------------
// Threads on several processors.

/// Handoffs a pair must make within HANDOFF_MS to show they run at the same time: two
/// threads taking turns on one processor hand off no faster than the timer tick.
const HANDOFFS: u64 = 1000;
const HANDOFF_MS: u64 = 1000;
/// The most worker threads a case starts; it starts one fewer than the processors online.
const MAX_WORKERS: usize = 3;

static SLOTS: [AtomicU64; MAX_WORKERS] = [const { AtomicU64::new(0) }; MAX_WORKERS];
/// 0 while the workers are being shown to run at once, 1 to run, 2 to stop.
static GO: AtomicU32 = AtomicU32::new(0);

/// The workers a case starts: one per processor beyond the first, at most MAX_WORKERS.
/// Skips when fewer than two processors are online.
fn workers_for() -> Result<usize, CaseError> {
    let cpus = processors()?;
    if cpus < 2 { return Err(CaseError::Skip(format!("{cpus} processor online; the case needs 2"))); }
    Ok((cpus - 1).min(MAX_WORKERS))
}

/// Worker side: answer handoffs on slot `i` until the case says to run or stop.
fn answer(i: usize) -> bool {
    let start = now_ms();
    let mut spins = 0u64;
    loop {
        let v = SLOTS[i].load(SeqCst);
        if v & 1 == 1 { SLOTS[i].store(v + 1, SeqCst); }
        match GO.load(SeqCst) {
            0 => {}
            1 => return true,
            _ => return false,
        }
        spins += 1;
        if spins % 4096 == 0 && now_ms().saturating_sub(start) >= 5000 { return false; }
        core::hint::spin_loop();
    }
}

/// Hand off HANDOFFS times with each of `n` workers, so each ran at the same time as
/// this thread; then let them run.
fn prove_parallel(n: usize, cpus: usize) -> CaseResult {
    for (i, slot) in SLOTS.iter().enumerate().take(n) {
        for k in 0..HANDOFFS {
            let odd = 2 * k + 1;
            slot.store(odd, SeqCst);
            if !spin_until(HANDOFF_MS, || slot.load(SeqCst) == odd + 1) {
                GO.store(2, SeqCst);
                return fail(format!("with {cpus} processors online, worker {i} made only {k} of {HANDOFFS} handoffs in {HANDOFF_MS} ms, so it never ran at the same time as the case"));
            }
        }
    }
    GO.store(1, SeqCst);
    Ok(())
}

// State of the two cases that change a page while other processors use it.
/// The page the workers touch this round.
static T_PAGE: AtomicU64 = AtomicU64::new(0);
/// The round the workers are in.
static T_ROUND: AtomicU32 = AtomicU32::new(0);
/// The round whose page has been unmapped or made read-only, once the call returned.
static T_CHANGED: AtomicU32 = AtomicU32::new(0);
/// Each worker's successful accesses this round.
static T_OK: [AtomicU64; MAX_WORKERS] = [const { AtomicU64::new(0) }; MAX_WORKERS];
/// The last round in which each worker's access faulted.
static T_FAULTED: [AtomicU32; MAX_WORKERS] = [const { AtomicU32::new(0) }; MAX_WORKERS];
/// Accesses that succeeded although they began after the change had returned.
static T_STALE: AtomicU64 = AtomicU64::new(0);
/// Loads that read a byte other than the round's own before the change.
static T_WRONG: AtomicU64 = AtomicU64::new(0);

/// Rounds of the cross-processor cases, and how many accesses each worker makes in a
/// round before the page is changed.
const ROUNDS: u32 = 50;
const WARM: u64 = 100;

/// A worker for the cross-processor cases: in each round, load from the page (`write`
/// false) or store to it (`write` true) until an access faults.
fn page_worker(i: usize, write: bool) {
    if !answer(i) { return; }
    let start = now_ms();
    let mut done = 0u32;
    loop {
        if GO.load(SeqCst) == 2 || now_ms().saturating_sub(start) > 8000 { return; }
        let round = T_ROUND.load(SeqCst);
        if round == done { core::hint::spin_loop(); continue; }
        let changed = T_CHANGED.load(SeqCst) == round;
        let page = T_PAGE.load(SeqCst) as *mut u8;
        let got = if write { store(at(page, 64 * i), round as u8).map(|_| round as u8) } else { load(page) };
        match got {
            Ok(v) => {
                T_OK[i].fetch_add(1, SeqCst);
                if changed { T_STALE.fetch_add(1, SeqCst); } else if v != round as u8 { T_WRONG.fetch_add(1, SeqCst); }
            }
            Err(_) => {
                T_FAULTED[i].store(round, SeqCst);
                done = round;
            }
        }
    }
}

/// Run ROUNDS rounds: `prepare` gives each round's page, `change` unmaps it or makes it
/// read-only while the workers use it. Returns the longest `change` in nanoseconds.
fn page_rounds(n: usize, mut prepare: impl FnMut(u32) -> Result<*mut u8, CaseError>, mut change: impl FnMut(*mut u8) -> i64, what: &str) -> Result<i64, CaseError> {
    let mut longest = 0i64;
    for round in 1..=ROUNDS {
        let page = prepare(round)?;
        for ok in T_OK.iter().take(n) { ok.store(0, SeqCst); }
        T_PAGE.store(page as u64, SeqCst);
        T_ROUND.store(round, SeqCst);
        if !spin_until(1000, || T_OK.iter().take(n).all(|ok| ok.load(SeqCst) >= WARM)) {
            return err(format!("round {round}: the workers did not each make {WARM} accesses to the page within a second"));
        }
        let t0 = mono();
        let r = change(page);
        let took = mono() - t0;
        T_CHANGED.store(round, SeqCst);
        zero(what, r)?;
        longest = longest.max(took);
        if !spin_until(1000, || T_FAULTED.iter().take(n).all(|f| f.load(SeqCst) == round)) {
            let late: Vec<usize> = (0..n).filter(|&i| T_FAULTED[i].load(SeqCst) != round).collect();
            return err(format!("round {round}: worker(s) {late:?} kept accessing the page for a second after {what} returned, with no fault ({} stale accesses so far)", T_STALE.load(SeqCst)));
        }
    }
    Ok(longest)
}

/// Start `n` page workers, show they run at once, run the rounds and stop them.
fn with_page_workers(write: bool, body: impl FnOnce(usize) -> Result<i64, CaseError>) -> Result<i64, CaseError> {
    catch_faults()?;
    let n = workers_for()?;
    let cpus = processors()?;
    let threads: Vec<_> = (0..n).map(|i| std::thread::spawn(move || page_worker(i, write))).collect();
    let result = prove_parallel(n, cpus).and_then(|_| body(n));
    GO.store(2, SeqCst);
    for t in threads { let _ = t.join(); }
    result
}

// ---------------------------------------------------------------------------
// anonymous: anonymous mmap & munmap.

fn an_private_zero() -> CaseResult {
    let len = 16 * PAGE;
    let p = anon(len)?;
    value("mapped", (len / 1024) as i64, "kb", None);
    want_bytes(p, len, 0, "a new MAP_PRIVATE anonymous mapping")?;
    fill(p, len, 1);
    want_pattern(p, len, 1, "a MAP_PRIVATE anonymous mapping after it was written")?;
    unmap(p, len)
}

fn an_shared_zero() -> CaseResult {
    let len = 16 * PAGE;
    let p = map(0, len, RW, SHARED, -1)?;
    value("mapped", (len / 1024) as i64, "kb", None);
    want_bytes(p, len, 0, "a new MAP_SHARED anonymous mapping")?;
    fill(p, len, 2);
    want_pattern(p, len, 2, "a MAP_SHARED anonymous mapping after it was written")?;
    unmap(p, len)
}

fn an_page_aligned() -> CaseResult {
    for len in [1usize, 4095, 4096, 4097] {
        let p = anon(len)?;
        check(p as usize % PAGE == 0, &format!("mmap of {len} bytes returned {:#x}, which is not page-aligned", p as usize))?;
        unmap(p, len)?;
    }
    Ok(())
}

fn an_partial_page() -> CaseResult {
    catch_faults()?;
    let p = anon(PAGE + 1)?;
    let last = at(p, 2 * PAGE - 1);
    loads("reading the last byte of the page a 4097-byte mapping ends in", last, Some(0))?;
    stores("writing the last byte of the page a 4097-byte mapping ends in", last, 0x5a)?;
    loads("reading it back", last, Some(0x5a))?;
    unmap(p, PAGE + 1)
}

fn an_distinct() -> CaseResult {
    let len = 4 * PAGE;
    let a = anon(len)?;
    let b = anon(len)?;
    let (a0, b0) = (a as usize, b as usize);
    check(a0 + len <= b0 || b0 + len <= a0, &format!("two 4-page mappings overlap: {a0:#x} and {b0:#x}"))?;
    fill(a, len, 3);
    fill_byte(b, len, 0xbb);
    want_pattern(a, len, 3, "the first mapping after the second was written")?;
    want_bytes(b, len, 0xbb, "the second mapping")?;
    unmap(a, len)?;
    unmap(b, len)
}

fn an_hint_occupied() -> CaseResult {
    let len = 4 * PAGE;
    let p = anon(len)?;
    fill(p, len, 4);
    let hint = at(p, PAGE) as u64;
    let r = mmap_raw(hint, PAGE as u64, RW, PRIVATE, -1, 0);
    if is_err(r) { return fail(format!("mmap with an occupied hint failed with {}", errname(-r))); }
    let q = r as usize;
    check(q + PAGE <= p as usize || q >= p as usize + len, &format!("mmap with hint {hint:#x} inside an existing mapping returned {q:#x}, inside that mapping"))?;
    want_pattern(p, len, 4, "the existing mapping after a mapping was requested at a hint inside it")?;
    unmap(q as *mut u8, PAGE)?;
    unmap(p, len)
}

fn an_hint_free() -> CaseResult {
    let addr = free_range(2 * PAGE)?;
    let r = mmap_raw(addr, PAGE as u64, RW, PRIVATE, -1, 0);
    if is_err(r) { return fail(format!("mmap with a free hint failed with {}", errname(-r))); }
    check(r as u64 == addr, &format!("mmap with the free, page-aligned hint {addr:#x} returned {}", shown(r)))?;
    unmap(r as u64 as *mut u8, PAGE)
}

fn an_fixed_replace() -> CaseResult {
    let len = 3 * PAGE;
    let p = anon(len)?;
    fill_byte(p, len, 0x22);
    let mid = at(p, PAGE);
    let r = mmap_raw(mid as u64, PAGE as u64, RW, PRIVATE | MAP_FIXED, -1, 0);
    check(r == mid as i64, &format!("MAP_FIXED at {:#x} returned {}", mid as u64, shown(r)))?;
    want_bytes(mid, PAGE, 0, "the page MAP_FIXED replaced")?;
    want_bytes(p, PAGE, 0x22, "the page before the replaced one")?;
    want_bytes(at(p, 2 * PAGE), PAGE, 0x22, "the page after the replaced one")?;
    fill_byte(mid, PAGE, 0x33);
    want_bytes(mid, PAGE, 0x33, "the replaced page after it was written")?;
    unmap(p, len)
}

fn an_fixed_unaligned() -> CaseResult {
    let p = anon(2 * PAGE)?;
    fill_byte(p, 2 * PAGE, 0x44);
    want_err("MAP_FIXED at a page address plus 1", mmap_raw(p as u64 + 1, PAGE as u64, RW, PRIVATE | MAP_FIXED, -1, 0), EINVAL)?;
    want_bytes(p, 2 * PAGE, 0x44, "the mapping at that address")?;
    unmap(p, 2 * PAGE)
}

fn an_noreplace_busy() -> CaseResult {
    let p = anon(2 * PAGE)?;
    fill_byte(p, 2 * PAGE, 0x55);
    let r = mmap_raw(p as u64, PAGE as u64, RW, PRIVATE | MAP_FIXED_NOREPLACE, -1, 0);
    if !is_err(r) && r != p as i64 { let _ = munmap(r as u64 as *mut u8, PAGE); }
    want_err("MAP_FIXED_NOREPLACE over an existing mapping", r, EEXIST)?;
    want_bytes(p, 2 * PAGE, 0x55, "the existing mapping")?;
    unmap(p, 2 * PAGE)
}

fn an_noreplace_free() -> CaseResult {
    let addr = free_range(PAGE)?;
    let r = mmap_raw(addr, PAGE as u64, RW, PRIVATE | MAP_FIXED_NOREPLACE, -1, 0);
    if is_err(r) { return fail(format!("MAP_FIXED_NOREPLACE at a free address failed with {}", errname(-r))); }
    check(r as u64 == addr, &format!("MAP_FIXED_NOREPLACE at the free address {addr:#x} returned {}", shown(r)))?;
    unmap(r as u64 as *mut u8, PAGE)
}

fn an_length_zero() -> CaseResult {
    want_err("mmap of length 0", mmap_raw(0, 0, RW, PRIVATE, -1, 0), EINVAL)
}

fn an_no_type() -> CaseResult {
    want_err("mmap with MAP_ANONYMOUS but neither MAP_SHARED nor MAP_PRIVATE", mmap_raw(0, PAGE as u64, RW, MAP_ANONYMOUS, -1, 0), EINVAL)
}

fn an_offset_unaligned() -> CaseResult {
    want_err("anonymous mmap with offset 100", mmap_raw(0, PAGE as u64, RW, PRIVATE, -1, 100), EINVAL)
}

fn an_ebadf() -> CaseResult {
    want_err("mmap of descriptor -1 without MAP_ANONYMOUS", mmap_raw(0, PAGE as u64, PROT_READ, MAP_PRIVATE, -1, 0), EBADF)?;
    let f = TempFile::new("ebadf", &[0u8; 16])?;
    let fd = f.open(O_RDONLY)?;
    close(fd);
    want_err("mmap of a closed descriptor", mmap_raw(0, PAGE as u64, PROT_READ, MAP_PRIVATE, fd, 0), EBADF)
}

fn an_enomem_space() -> CaseResult {
    let r = mmap_raw(0, 1u64 << 60, PROT_NONE, PRIVATE, -1, 0);
    if !is_err(r) { let _ = munmap(r as u64 as *mut u8, 1 << 60); }
    want_err("mmap of 2^60 bytes", r, ENOMEM)
}

fn an_enomem_fixed() -> CaseResult {
    let addr = 1u64 << 56;
    let r = mmap_raw(addr, PAGE as u64, RW, PRIVATE | MAP_FIXED, -1, 0);
    if !is_err(r) { let _ = munmap(r as u64 as *mut u8, PAGE); }
    want_err("MAP_FIXED at 2^56, past the end of the user address space", r, ENOMEM)
}

fn an_rlimit_as() -> CaseResult {
    let big = 512 * MIB;
    let free = mmap_raw(0, big as u64, PROT_NONE, PRIVATE, -1, 0);
    if is_err(free) { return fail(format!("with no RLIMIT_AS, a 512 MiB PROT_NONE mapping failed with {}", errname(-free))); }
    unmap(free as u64 as *mut u8, big)?;
    // The child allocates nothing once its limit is low.
    let mut child = Child::start(|| {
        if set_limit(RLIMIT_AS, 256 * MIB as u64, RLIM_INFINITY) != 0 { return 1; }
        let r = mmap_raw(0, big as u64, PROT_NONE, PRIVATE, -1, 0);
        if r != -ENOMEM { return 2; }
        let r = mmap_raw(0, 16 * MIB as u64, RW, PRIVATE, -1, 0);
        if is_err(r) { return 3; }
        0
    })?;
    child.finish("the child", &[
        "setrlimit(RLIMIT_AS, 256 MiB) failed",
        "with RLIMIT_AS at 256 MiB, a 512 MiB mapping did not fail with ENOMEM",
        "with RLIMIT_AS at 256 MiB, a 16 MiB mapping failed",
    ])
}

fn an_munmap_whole() -> CaseResult {
    catch_faults()?;
    let p = anon(2 * PAGE)?;
    fill_byte(p, 2 * PAGE, 0x66);
    unmap(p, 2 * PAGE)?;
    load_faults("reading the first page after munmap", p, SIGSEGV, Some(SEGV_MAPERR))?;
    load_faults("reading the second page after munmap", at(p, PAGE + 5), SIGSEGV, Some(SEGV_MAPERR))
}

fn an_munmap_middle() -> CaseResult {
    catch_faults()?;
    let len = 3 * PAGE;
    let p = anon(len)?;
    fill(p, len, 5);
    unmap(at(p, PAGE), PAGE)?;
    want_pattern(p, PAGE, 5, "the first page after the middle one was unmapped")?;
    for off in 2 * PAGE..3 * PAGE {
        check(peek(at(p, off)) == pat(off, 5), &format!("byte {off} of the last page changed when the middle page was unmapped"))?;
    }
    load_faults("reading the unmapped middle page", at(p, PAGE + 9), SIGSEGV, Some(SEGV_MAPERR))?;
    unmap(p, PAGE)?;
    unmap(at(p, 2 * PAGE), PAGE)
}

fn an_munmap_head_tail() -> CaseResult {
    catch_faults()?;
    let len = 4 * PAGE;
    let p = anon(len)?;
    fill(p, len, 6);
    unmap(p, PAGE)?;
    unmap(at(p, 3 * PAGE), PAGE)?;
    for off in PAGE..3 * PAGE {
        check(peek(at(p, off)) == pat(off, 6), &format!("byte {off} changed when the first and last pages were unmapped"))?;
    }
    load_faults("reading the unmapped first page", p, SIGSEGV, Some(SEGV_MAPERR))?;
    load_faults("reading the unmapped last page", at(p, 3 * PAGE), SIGSEGV, Some(SEGV_MAPERR))?;
    unmap(at(p, PAGE), 2 * PAGE)
}

fn an_munmap_unmapped() -> CaseResult {
    let addr = free_range(4 * PAGE)?;
    zero("munmap of a range with nothing mapped", munmap(addr as *mut u8, 4 * PAGE))
}

fn an_munmap_span() -> CaseResult {
    catch_faults()?;
    let r = map(0, 5 * PAGE, PROT_NONE, PRIVATE, -1)?;
    let a = mmap_raw(r as u64, 2 * PAGE as u64, RW, PRIVATE | MAP_FIXED, -1, 0);
    let b = mmap_raw(r as u64 + 3 * PAGE as u64, 2 * PAGE as u64, RW, PRIVATE | MAP_FIXED, -1, 0);
    check(a == r as i64 && b == r as i64 + 3 * PAGE as i64, "placing two mappings with MAP_FIXED failed")?;
    unmap(at(r, 2 * PAGE), PAGE)?;
    poke(r, 1);
    poke(at(r, 3 * PAGE), 2);
    zero("munmap across both mappings and the gap between them", munmap(r, 5 * PAGE))?;
    load_faults("reading the first mapping", r, SIGSEGV, Some(SEGV_MAPERR))?;
    load_faults("reading the second mapping", at(r, 4 * PAGE + 1), SIGSEGV, Some(SEGV_MAPERR))
}

fn an_munmap_unaligned() -> CaseResult {
    let p = anon(2 * PAGE)?;
    fill_byte(p, 2 * PAGE, 0x77);
    want_err("munmap at a page address plus 1", munmap(at(p, 1), PAGE), EINVAL)?;
    want_bytes(p, 2 * PAGE, 0x77, "the mapping")?;
    unmap(p, 2 * PAGE)
}

fn an_munmap_zero() -> CaseResult {
    let p = anon(PAGE)?;
    want_err("munmap of length 0", munmap(p, 0), EINVAL)?;
    unmap(p, PAGE)
}

fn an_remap_zero() -> CaseResult {
    let p = anon(2 * PAGE)?;
    fill_byte(p, 2 * PAGE, 0x88);
    unmap(at(p, PAGE), PAGE)?;
    let r = mmap_raw(p as u64 + PAGE as u64, PAGE as u64, RW, PRIVATE | MAP_FIXED, -1, 0);
    check(r == p as i64 + PAGE as i64, &format!("MAP_FIXED into the hole returned {}", shown(r)))?;
    want_bytes(at(p, PAGE), PAGE, 0, "the page mapped into the hole")?;
    want_bytes(p, PAGE, 0x88, "the page before the hole")?;
    unmap(p, 2 * PAGE)
}

fn an_sparse() -> CaseResult {
    let len = 256 * MIB;
    let before = resident()?;
    let p = anon(len)?;
    for i in 0..256 { poke(at(p, i * MIB), i as u8 | 1); }
    let after = resident()?;
    let grew = after - before;
    value("mapped", (len / MIB) as i64, "mb", None);
    value("resident-pages", grew, "", Some((256, 320)));
    for i in 0..256 {
        check(peek(at(p, i * MIB)) == i as u8 | 1, &format!("the byte written at {i} MiB did not read back"))?;
    }
    check((256..=320).contains(&grew), &format!("touching 256 pages of a 256 MiB mapping made {grew} pages resident, not about 256"))?;
    unmap(p, len)
}

fn an_page_by_page() -> CaseResult {
    let pages = 4096;
    let len = pages * PAGE;
    let p = anon(len)?;
    let before = resident()?;
    let t0 = mono();
    for i in 0..pages {
        poke(at(p, i * PAGE), i as u8);
        poke(at(p, i * PAGE + PAGE - 1), (i >> 8) as u8);
    }
    let took = mono() - t0;
    let grew = resident()? - before;
    value("fault", took / pages as i64, "ns", None);
    value("resident-pages", grew, "", Some((pages as i64, pages as i64 + 64)));
    for i in 0..pages {
        check(peek(at(p, i * PAGE)) == i as u8 && peek(at(p, i * PAGE + PAGE - 1)) == (i >> 8) as u8, &format!("page {i} of 4096 did not keep its own contents"))?;
    }
    check(grew >= pages as i64, &format!("after touching all 4096 pages, only {grew} more were resident"))?;
    unmap(p, len)
}

fn an_churn() -> CaseResult {
    let rounds = 100;
    let before = mem_free_kb()?;
    let rss = resident()?;
    for round in 0..rounds {
        let p = anon(MIB)?;
        for i in 0..MIB / PAGE { poke(at(p, i * PAGE), round as u8); }
        unmap(p, MIB)?;
    }
    let mut drop = 0;
    let settled = until(1000, || { drop = before - mem_free_kb().unwrap_or(0); drop <= 4096 });
    let rss_grew = resident()? - rss;
    value("free-drop", drop, "kb", Some((-4096, 4096)));
    value("resident-grew", rss_grew, "", Some((0, 64)));
    check(settled, &format!("after mapping, touching and unmapping 1 MiB {rounds} times, free memory is {drop} kB lower than before"))?;
    check(rss_grew <= 64, &format!("after {rounds} map-touch-unmap rounds the process has {rss_grew} more resident pages"))
}

fn an_many() -> CaseResult {
    let n = 1000;
    let mut maps: Vec<usize> = Vec::with_capacity(n);
    for i in 0..n {
        let p = anon(PAGE)?;
        poke(p, i as u8);
        poke(at(p, 1), (i >> 8) as u8);
        maps.push(p as usize);
    }
    value("mappings", n as i64, "", None);
    let mut sorted = maps.clone();
    sorted.sort_unstable();
    check(sorted.windows(2).all(|w| w[0] + PAGE <= w[1]), "two of 1000 one-page mappings share an address")?;
    for (i, &p) in maps.iter().enumerate() {
        let p = p as *mut u8;
        check(peek(p) == i as u8 && peek(at(p, 1)) == (i >> 8) as u8, &format!("mapping {i} of 1000 lost its contents"))?;
    }
    for &p in &maps { unmap(p as *mut u8, PAGE)?; }
    Ok(())
}

fn an_unmap_cpus() -> CaseResult {
    let longest = with_page_workers(false, |n| {
        page_rounds(n, |round| {
            let p = anon(PAGE)?;
            fill_byte(p, PAGE, round as u8);
            Ok(p)
        }, |p| munmap(p, PAGE), "munmap")
    })?;
    let (stale, wrong) = (T_STALE.load(SeqCst), T_WRONG.load(SeqCst));
    value("stale-reads", stale as i64, "", Some((0, 0)));
    value("munmap", longest / 1000, "us", None);
    value("maperr", MAPERR.load(SeqCst) as i64, "", None);
    check(wrong == 0, &format!("{wrong} reads from a still-mapped page returned another page's byte"))?;
    check(stale == 0, &format!("{stale} reads begun after munmap returned read the unmapped page instead of faulting"))?;
    check(ACCERR.load(SeqCst) == 0, "a read of an unmapped page faulted with SEGV_ACCERR, not SEGV_MAPERR")
}

// ---------------------------------------------------------------------------
// protection: mprotect & faults.

fn pr_none_read() -> CaseResult {
    catch_faults()?;
    let p = map(0, PAGE, PROT_NONE, PRIVATE, -1)?;
    load_faults("reading a PROT_NONE page", at(p, 0x123), SIGSEGV, Some(SEGV_ACCERR))
}

fn pr_none_write() -> CaseResult {
    catch_faults()?;
    let p = map(0, PAGE, PROT_NONE, PRIVATE, -1)?;
    store_faults("writing a PROT_NONE page", at(p, 0x321), SEGV_ACCERR)
}

fn pr_readonly_write() -> CaseResult {
    catch_faults()?;
    let p = anon(PAGE)?;
    fill_byte(p, PAGE, 0x5a);
    protect(p, PAGE, PROT_READ)?;
    store_faults("writing a PROT_READ page", at(p, 7), SEGV_ACCERR)?;
    loads("reading the byte the write faulted on", at(p, 7), Some(0x5a))?;
    loads("reading the page", p, Some(0x5a))
}

fn pr_unmapped_maperr() -> CaseResult {
    catch_faults()?;
    let addr = free_range(PAGE)? as *mut u8;
    load_faults("reading an address with nothing mapped", at(addr, 0x40), SIGSEGV, Some(SEGV_MAPERR))
}

fn pr_handler_retry() -> CaseResult {
    catch_faults()?;
    let pages = 64;
    let p = map(0, pages * PAGE, PROT_NONE, PRIVATE, -1)?;
    FAULTS.store(0, SeqCst);
    REPAIR.store(1, SeqCst);
    for i in 0..pages { poke(at(p, i * PAGE + 1), i as u8 + 1); }
    REPAIR.store(0, SeqCst);
    let faults = FAULTS.load(SeqCst) as i64;
    value("faults", faults, "", Some((pages as i64, pages as i64)));
    for i in 0..pages {
        check(peek(at(p, i * PAGE + 1)) == i as u8 + 1, &format!("the write to page {i} that its handler let through did not stick"))?;
    }
    check(faults == pages as i64, &format!("writing 64 PROT_NONE pages, each made writable by the handler, took {faults} faults"))
}

fn pr_make_readonly() -> CaseResult {
    catch_faults()?;
    let p = anon(2 * PAGE)?;
    fill(p, 2 * PAGE, 7);
    protect(p, 2 * PAGE, PROT_READ)?;
    for off in [0, 100, PAGE - 1, PAGE, 2 * PAGE - 1] { loads("reading the read-only mapping", at(p, off), Some(pat(off, 7)))?; }
    store_faults("writing the read-only mapping", at(p, PAGE + 3), SEGV_ACCERR)
}

fn pr_make_writable() -> CaseResult {
    catch_faults()?;
    let p = anon(PAGE)?;
    fill(p, PAGE, 8);
    protect(p, PAGE, PROT_READ)?;
    protect(p, PAGE, RW)?;
    stores("writing the page made writable again", at(p, 9), 0xab)?;
    loads("reading it back", at(p, 9), Some(0xab))?;
    loads("reading a byte not written", at(p, 10), Some(pat(10, 8)))
}

fn pr_none_keeps() -> CaseResult {
    catch_faults()?;
    let p = anon(PAGE)?;
    fill(p, PAGE, 9);
    protect(p, PAGE, PROT_NONE)?;
    load_faults("reading the page while PROT_NONE", at(p, 50), SIGSEGV, Some(SEGV_ACCERR))?;
    protect(p, PAGE, RW)?;
    want_pattern(p, PAGE, 9, "the page after PROT_NONE and back")
}

fn pr_guard_page() -> CaseResult {
    catch_faults()?;
    let p = anon(3 * PAGE)?;
    fill_byte(p, 3 * PAGE, 0x11);
    protect(at(p, PAGE), PAGE, PROT_NONE)?;
    loads("reading the byte just below the guard page", at(p, PAGE - 1), Some(0x11))?;
    stores("writing the byte just below the guard page", at(p, PAGE - 1), 0x12)?;
    loads("reading the byte just above the guard page", at(p, 2 * PAGE), Some(0x11))?;
    load_faults("reading the guard page's first byte", at(p, PAGE), SIGSEGV, Some(SEGV_ACCERR))?;
    load_faults("reading the guard page's last byte", at(p, 2 * PAGE - 1), SIGSEGV, Some(SEGV_ACCERR))
}

/// Each page of `len_pages` at `p` must take a store exactly when `writable(page)`.
fn want_writable(p: *mut u8, len_pages: usize, writable: impl Fn(usize) -> bool) -> CaseResult {
    for page in 0..len_pages {
        let q = at(p, page * PAGE + 10);
        if writable(page) { stores(&format!("writing page {page}"), q, page as u8)?; } else { store_faults(&format!("writing read-only page {page}"), q, SEGV_ACCERR)?; }
        loads(&format!("reading page {page}"), q, None)?;
    }
    Ok(())
}

fn pr_partial() -> CaseResult {
    catch_faults()?;
    let p = anon(4 * PAGE)?;
    protect(p, PAGE, PROT_READ)?;
    want_writable(p, 4, |page| page != 0)
}

fn pr_partial_middle() -> CaseResult {
    catch_faults()?;
    let p = anon(6 * PAGE)?;
    protect(at(p, 2 * PAGE), 2 * PAGE, PROT_READ)?;
    protect(at(p, 3 * PAGE), PAGE, RW)?;
    want_writable(p, 6, |page| page != 2)
}

fn pr_unaligned() -> CaseResult {
    catch_faults()?;
    let p = anon(PAGE)?;
    want_err("mprotect at a page address plus 1", mprotect(at(p, 1), PAGE, PROT_READ), EINVAL)?;
    stores("writing the page after the refused mprotect", p, 1)
}

fn pr_unmapped_enomem() -> CaseResult {
    let p = anon(2 * PAGE)?;
    unmap(at(p, PAGE), PAGE)?;
    want_err("mprotect of a mapped page and the unmapped one after it", mprotect(p, 2 * PAGE, PROT_READ), ENOMEM)
}

fn pr_bad_prot() -> CaseResult {
    let p = anon(PAGE)?;
    want_err("mprotect with protection bit 0x100", mprotect(p, PAGE, 0x100), EINVAL)
}

fn pr_file_eacces() -> CaseResult {
    let f = TempFile::new("eacces", &[1u8; PAGE])?;
    let fd = f.open(O_RDONLY)?;
    let p = map(0, PAGE, PROT_READ, MAP_SHARED, fd)?;
    want_err("mprotect(PROT_READ|PROT_WRITE) of a MAP_SHARED mapping of a read-only descriptor", mprotect(p, PAGE, RW), EACCES)
}

fn pr_exec_denied() -> CaseResult {
    catch_faults()?;
    let p = anon(PAGE)?;
    for (i, &b) in CODE42.iter().enumerate() { poke(at(p, i), b); }
    call_faults("calling into a PROT_READ|PROT_WRITE mapping", p)
}

fn pr_exec_file() -> CaseResult {
    catch_faults()?;
    let f = TempFile::new("exec", &code_page())?;
    let p = map(0, PAGE, PROT_READ | PROT_EXEC, MAP_PRIVATE, f.fd)?;
    runs("calling the code in a PROT_READ|PROT_EXEC file mapping", p)?;
    protect(p, PAGE, PROT_READ)?;
    call_faults("calling it after mprotect to PROT_READ", p)
}

fn pr_exec_add() -> CaseResult {
    catch_faults()?;
    let f = TempFile::new("exec-add", &code_page())?;
    let p = map(0, PAGE, PROT_READ, MAP_PRIVATE, f.fd)?;
    call_faults("calling the code in a PROT_READ file mapping", p)?;
    protect(p, PAGE, PROT_READ | PROT_EXEC)?;
    runs("calling it after mprotect to PROT_READ|PROT_EXEC", p)
}

fn pr_exec_jit() -> CaseResult {
    catch_faults()?;
    let p = anon(PAGE)?;
    for (i, &b) in CODE42.iter().enumerate() { poke(at(p, i), b); }
    // SAFETY: the cache maintenance touches only the line holding `p`; a trap in it is
    // answered by `on_fault`.
    if unsafe { mem_clear_cache(p) } != 0 {
        return fail(format!("the user-mode cache maintenance a JIT does raised {}", fault_text(last_fault())));
    }
    protect(p, PAGE, PROT_READ | PROT_EXEC)?;
    runs("calling the code after mprotect to PROT_READ|PROT_EXEC", p)
}

fn pr_fork_inherits() -> CaseResult {
    catch_faults()?;
    let p = anon(2 * PAGE)?;
    fill_byte(p, 2 * PAGE, 0x3c);
    protect(p, PAGE, PROT_NONE)?;
    protect(at(p, PAGE), PAGE, PROT_READ)?;
    let mut child = Child::start(|| {
        match load(p) { Err(f) if f.sig == SIGSEGV && f.code == SEGV_ACCERR => {} _ => return 1 }
        match store(at(p, PAGE), 1) { Err(f) if f.sig == SIGSEGV && f.code == SEGV_ACCERR => {} _ => return 2 }
        match load(at(p, PAGE)) { Ok(0x3c) => 0, _ => 3 }
    })?;
    child.finish("the child", &[
        "in the child, reading the PROT_NONE page did not raise SIGSEGV with SEGV_ACCERR",
        "in the child, writing the PROT_READ page did not raise SIGSEGV with SEGV_ACCERR",
        "in the child, the PROT_READ page did not read back its contents",
    ])
}

fn pr_tlb_cpus() -> CaseResult {
    let page = anon(PAGE)?;
    let longest = with_page_workers(true, |n| {
        page_rounds(n, |_| { protect(page, PAGE, RW)?; Ok(page) }, |p| mprotect(p, PAGE, PROT_READ), "mprotect(PROT_READ)")
    })?;
    let stale = T_STALE.load(SeqCst);
    value("stale-writes", stale as i64, "", Some((0, 0)));
    value("mprotect", longest / 1000, "us", None);
    value("accerr", ACCERR.load(SeqCst) as i64, "", None);
    check(stale == 0, &format!("{stale} writes begun after mprotect(PROT_READ) returned succeeded on another processor"))?;
    check(MAPERR.load(SeqCst) == 0, "a write to the read-only page faulted with SEGV_MAPERR, not SEGV_ACCERR")
}

// ---------------------------------------------------------------------------
// shared: shared mappings across fork.

fn sh_anon_child_sees() -> CaseResult {
    let p = map(0, PAGE, RW, SHARED, -1)?;
    let (r, w) = pipe()?;
    let mut child = Child::start(|| {
        close(w);
        if recv(r, WAIT_MS) != Some(1) { return 1; }
        if peek(p) != 0x77 || peek(at(p, PAGE - 1)) != 0x78 { return 2; }
        0
    })?;
    close(r);
    poke(p, 0x77);
    poke(at(p, PAGE - 1), 0x78);
    send(w, 1);
    child.finish("the child", &["the child heard nothing from the parent", "the child did not see the parent's stores made after fork"])
}

fn sh_anon_parent_sees() -> CaseResult {
    let p = map(0, PAGE, RW, SHARED, -1)?;
    let mut child = Child::start(|| { poke(p, 0x66); poke(at(p, 2000), 0x67); 0 })?;
    child.finish("the child", &[])?;
    check(peek(p) == 0x66 && peek(at(p, 2000)) == 0x67, "the parent did not see the child's stores to a MAP_SHARED anonymous page")
}

fn sh_pingpong() -> CaseResult {
    let rounds = 100u32;
    let p = map(0, PAGE, RW, SHARED, -1)? as *mut u32;
    let (to_child_r, to_child_w) = pipe()?;
    let (to_parent_r, to_parent_w) = pipe()?;
    let mut child = Child::start(|| {
        close(to_child_w);
        close(to_parent_r);
        for k in 0..rounds {
            if recv(to_child_r, WAIT_MS) != Some(1) { return 1; }
            // SAFETY: `p` is the shared page.
            if unsafe { core::ptr::read_volatile(p) } != 2 * k + 1 { return 2; }
            unsafe { core::ptr::write_volatile(p, 2 * k + 2) };
            send(to_parent_w, 1);
        }
        0
    })?;
    close(to_child_r);
    close(to_parent_w);
    let mut seen = 0;
    for k in 0..rounds {
        // SAFETY: `p` is the shared page.
        unsafe { core::ptr::write_volatile(p, 2 * k + 1) };
        send(to_child_w, 1);
        if recv(to_parent_r, WAIT_MS) != Some(1) { break; }
        if unsafe { core::ptr::read_volatile(p) } != 2 * k + 2 { break; }
        seen = k + 1;
    }
    value("rounds", seen as i64, "", Some((rounds as i64, rounds as i64)));
    child.finish("the child", &["the child stopped hearing from the parent", "the child did not see the parent's latest store"])?;
    check(seen == rounds, &format!("the parent saw the child's store in only {seen} of {rounds} rounds"))
}

fn sh_anon_atomic() -> CaseResult {
    let adds = 100_000u64;
    let p = map(0, PAGE, RW, SHARED, -1)?;
    // SAFETY: the page is mapped, aligned and lives until the case ends.
    let counter = unsafe { &*(p as *const AtomicU64) };
    let mut child = Child::start(|| { for _ in 0..adds { counter.fetch_add(1, SeqCst); } 0 })?;
    for _ in 0..adds { counter.fetch_add(1, SeqCst); }
    child.finish("the child", &[])?;
    let total = counter.load(SeqCst);
    value("total", total as i64, "", Some((2 * adds as i64, 2 * adds as i64)));
    check(total == 2 * adds, &format!("two processes each adding 1 {adds} times left {total}"))
}

fn sh_private_child_isolated() -> CaseResult {
    let p = anon(PAGE)?;
    poke(p, 0x10);
    let mut child = Child::start(|| {
        if peek(p) != 0x10 { return 1; }
        poke(p, 0x20);
        if peek(p) != 0x20 { return 2; }
        0
    })?;
    child.finish("the child", &["the child did not see the contents from before fork", "the child's own store did not read back"])?;
    check(peek(p) == 0x10, &format!("the parent's MAP_PRIVATE page reads {:#04x} after the child stored 0x20 to its copy", peek(p)))
}

fn sh_private_parent_isolated() -> CaseResult {
    let p = anon(PAGE)?;
    poke(p, 0x10);
    let (r, w) = pipe()?;
    let mut child = Child::start(|| {
        close(w);
        if recv(r, WAIT_MS) != Some(1) { return 1; }
        if peek(p) != 0x10 { return 2; }
        0
    })?;
    close(r);
    poke(p, 0x30);
    send(w, 1);
    child.finish("the child", &["the child heard nothing from the parent", "the child saw the parent's store, made after fork, to a MAP_PRIVATE page"])
}

fn sh_private_cow_many() -> CaseResult {
    let pages = 64;
    let len = pages * PAGE;
    let p = anon(len)?;
    fill(p, len, 10);
    let (to_child_r, to_child_w) = pipe()?;
    let (to_parent_r, to_parent_w) = pipe()?;
    let mut child = Child::start(|| {
        close(to_child_w);
        close(to_parent_r);
        if pattern_gap(p, len, 10).is_some() { return 1; }
        fill(p, len, 11);
        if pattern_gap(p, len, 11).is_some() { return 2; }
        send(to_parent_w, 1);
        if recv(to_child_r, WAIT_MS) != Some(1) { return 3; }
        if pattern_gap(p, len, 11).is_some() { return 4; }
        0
    })?;
    close(to_child_r);
    close(to_parent_w);
    let t0 = mono();
    for i in 0..pages { poke(at(p, i * PAGE), pat(i * PAGE, 12)); }
    let took = mono() - t0;
    fill(p, len, 12);
    let heard = recv(to_parent_r, WAIT_MS) == Some(1);
    let kept = pattern_gap(p, len, 12);
    send(to_child_w, 1);
    value("copy", took / pages as i64, "ns", None);
    child.finish("the child", &[
        "the child did not see the contents from before fork",
        "the child's own stores did not read back",
        "the child heard nothing from the parent",
        "the child's copy changed when the parent wrote its own",
    ])?;
    check(heard, "the parent heard nothing from the child")?;
    check(kept.is_none(), &format!("the parent's copy changed at byte {} when the child wrote its own", kept.unwrap_or(0)))
}

fn sh_file_child_sees() -> CaseResult {
    let f = TempFile::new("child-sees", &[0u8; PAGE])?;
    let p = map(0, PAGE, RW, MAP_SHARED, f.fd)?;
    let mut child = Child::start(|| { for (i, &b) in b"child".iter().enumerate() { poke(at(p, 100 + i), b); } 0 })?;
    child.finish("the child", &[])?;
    let seen: Vec<u8> = (0..5).map(|i| peek(at(p, 100 + i))).collect();
    check(seen == b"child", "the parent's MAP_SHARED file mapping did not show the child's store")?;
    check(pread_n(f.fd, 5, 100)? == b"child", "read() of the file did not return the child's store to its MAP_SHARED mapping")
}

fn sh_file_independent() -> CaseResult {
    let f = TempFile::new("independent", &[0u8; PAGE])?;
    let p = map(0, PAGE, RW, MAP_SHARED, f.fd)?;
    let (to_child_r, to_child_w) = pipe()?;
    let (to_parent_r, to_parent_w) = pipe()?;
    let path = f.path.clone();
    let mut child = Child::start(|| {
        close(to_child_w);
        close(to_parent_r);
        let Ok(fd) = open_raw(&path, O_RDWR) else { return 1 };
        let q = mmap_raw(0, PAGE as u64, RW, MAP_SHARED, fd, 0);
        if is_err(q) { return 2; }
        let q = q as u64 as *mut u8;
        poke(q, b'C');
        send(to_parent_w, 1);
        if recv(to_child_r, WAIT_MS) != Some(1) { return 3; }
        if peek(at(q, 1)) != b'P' { return 4; }
        0
    })?;
    close(to_child_r);
    close(to_parent_w);
    let heard = recv(to_parent_r, WAIT_MS) == Some(1);
    let saw = peek(p);
    poke(at(p, 1), b'P');
    send(to_child_w, 1);
    child.finish("the child", &[
        "the child could not open the file",
        "the child could not map the file",
        "the child heard nothing from the parent",
        "the child's own mapping did not show the parent's store",
    ])?;
    check(heard, "the parent heard nothing from the child")?;
    check(saw == b'C', "the parent's mapping did not show the store the child made through its own mapping of the file")
}

fn sh_file_msync_read() -> CaseResult {
    let len = 2 * PAGE;
    let f = TempFile::new("msync-read", &vec![0u8; len])?;
    let p = map(0, len, RW, MAP_SHARED, f.fd)?;
    fill(p, len, 13);
    zero("msync(MS_SYNC)", msync(p, len, MS_SYNC))?;
    let back = pread_n(f.fd, len, 0)?;
    check(back.len() == len, &format!("read() returned {} bytes of a {len}-byte file", back.len()))?;
    match (0..len).find(|&off| back[off] != pat(off, 13)) {
        None => Ok(()),
        Some(off) => fail(format!("after msync(MS_SYNC), read() returns {:#04x} at byte {off}, not the {:#04x} stored there", back[off], pat(off, 13))),
    }
}

fn sh_file_write_seen() -> CaseResult {
    let f = TempFile::new("write-seen", &[0u8; PAGE])?;
    let p = map(0, PAGE, PROT_READ, MAP_SHARED, f.fd)?;
    check(peek(at(p, 50)) == 0, "a new mapping of a file of zeros did not read zero")?;
    pwrite_all(f.fd, b"fresh", 50)?;
    let seen: Vec<u8> = (0..5).map(|i| peek(at(p, 50 + i))).collect();
    check(seen == b"fresh", "a write() to the file was not seen through its MAP_SHARED mapping")
}

fn sh_file_private() -> CaseResult {
    let f = TempFile::new("private", &[b'f'; PAGE])?;
    let p = map(0, PAGE, RW, MAP_PRIVATE, f.fd)?;
    fill_byte(p, 10, b'x');
    let path = f.path.clone();
    let mut child = Child::start(|| {
        let Ok(fd) = open_raw(&path, O_RDONLY) else { return 1 };
        let q = mmap_raw(0, PAGE as u64, PROT_READ, MAP_SHARED, fd, 0);
        if is_err(q) { return 2; }
        if byte_gap(q as u64 as *mut u8, 10, b'f').is_some() { return 3; }
        0
    })?;
    child.finish("the child", &["the child could not open the file", "the child could not map the file", "another process's MAP_SHARED mapping showed the stores made to a MAP_PRIVATE mapping"])?;
    check(pread_n(f.fd, 10, 0)? == [b'f'; 10], "stores to a MAP_PRIVATE file mapping were written to the file")?;
    want_bytes(p, 10, b'x', "the MAP_PRIVATE mapping's own stores")
}

fn sh_child_unmaps() -> CaseResult {
    catch_faults()?;
    let p = map(0, PAGE, RW, SHARED, -1)?;
    poke(p, 1);
    let mut child = Child::start(|| {
        if munmap(p, PAGE) != 0 { return 1; }
        match load(p) { Err(f) if f.sig == SIGSEGV => 0, _ => 2 }
    })?;
    child.finish("the child", &["the child's munmap of the inherited mapping failed", "the child could still read the page after unmapping it"])?;
    check(peek(p) == 1, "the parent's view of the shared page changed when the child unmapped it")?;
    poke(p, 2);
    check(peek(p) == 2, "the parent's store to the shared page did not stick after the child unmapped it")
}

fn sh_parent_unmaps() -> CaseResult {
    let p = map(0, PAGE, RW, SHARED, -1)?;
    let (r, w) = pipe()?;
    let mut child = Child::start(|| {
        close(w);
        if recv(r, WAIT_MS) != Some(1) { return 1; }
        if peek(p) != b'b' { return 2; }
        poke(p, b'c');
        if peek(p) != b'c' { return 3; }
        0
    })?;
    close(r);
    poke(p, b'b');
    unmap(p, PAGE)?;
    send(w, 1);
    child.finish("the child", &[
        "the child heard nothing from the parent",
        "after the parent unmapped the shared page, the child did not read the parent's last store",
        "the child's own store to the page did not stick",
    ])
}

fn sh_eof_zero_tail() -> CaseResult {
    let f = TempFile::new("eof-tail", &[b'e'; 100])?;
    let p = map(0, PAGE, RW, MAP_SHARED, f.fd)?;
    want_bytes(p, 100, b'e', "the file's bytes in the mapping")?;
    want_bytes(at(p, 100), PAGE - 100, 0, "the part of the page past the end of the file")?;
    poke(at(p, 200), b'z');
    zero("msync(MS_SYNC)", msync(p, PAGE, MS_SYNC))?;
    let end = sc(nr::LSEEK, &[f.fd as u64, 0, SEEK_END]);
    check(end == 100, &format!("after a store past its end and msync, the 100-byte file is {} bytes", shown(end)))
}

fn sh_eof_sigbus() -> CaseResult {
    catch_faults()?;
    let f = TempFile::new("eof-sigbus", &[b'e'; 100])?;
    let p = map(0, 2 * PAGE, PROT_READ, MAP_SHARED, f.fd)?;
    loads("reading the file's first byte", p, Some(b'e'))?;
    load_faults("reading the page past the end of the file", at(p, PAGE + 8), SIGBUS, None)
}

fn sh_eof_truncated() -> CaseResult {
    catch_faults()?;
    let f = TempFile::new("eof-truncated", &[b't'; 2 * PAGE])?;
    let p = map(0, 2 * PAGE, PROT_READ, MAP_SHARED, f.fd)?;
    loads("reading the second page before truncation", at(p, PAGE), Some(b't'))?;
    zero("ftruncate to 100 bytes", sc(nr::FTRUNCATE, &[f.fd as u64, 100]))?;
    load_faults("reading the second page after the file was truncated to 100 bytes", at(p, PAGE + 8), SIGBUS, None)?;
    loads("reading a byte still in the file", at(p, 50), Some(b't'))
}

fn sh_write_only_fd() -> CaseResult {
    let f = TempFile::new("write-only", &[0u8; PAGE])?;
    let fd = f.open(O_WRONLY)?;
    want_err("mmap of a descriptor opened O_WRONLY", mmap_raw(0, PAGE as u64, PROT_READ, MAP_SHARED, fd, 0), EACCES)
}

fn sh_readonly_shared_write() -> CaseResult {
    let f = TempFile::new("ro-shared", &[0u8; PAGE])?;
    let fd = f.open(O_RDONLY)?;
    want_err("mmap(PROT_READ|PROT_WRITE, MAP_SHARED) of a descriptor opened O_RDONLY", mmap_raw(0, PAGE as u64, RW, MAP_SHARED, fd, 0), EACCES)
}

fn sh_readonly_private_write() -> CaseResult {
    let f = TempFile::new("ro-private", &[b'r'; PAGE])?;
    let fd = f.open(O_RDONLY)?;
    let p = map(0, PAGE, RW, MAP_PRIVATE, fd)?;
    fill_byte(p, 16, b'w');
    want_bytes(p, 16, b'w', "the private copy")?;
    check(pread_n(f.fd, 16, 0)? == [b'r'; 16], "stores to a MAP_PRIVATE mapping of a read-only descriptor reached the file")
}

fn sh_pipe_enodev() -> CaseResult {
    let (r, _w) = pipe()?;
    want_err("mmap of a pipe", mmap_raw(0, PAGE as u64, PROT_READ, MAP_SHARED, r, 0), ENODEV)
}

// ---------------------------------------------------------------------------
// brk: brk & sbrk.

fn brk(addr: u64) -> u64 { sc(nr::BRK, &[addr]) as u64 }
fn cur_brk() -> u64 { brk(0) }
fn page_up(a: u64) -> u64 { (a + PAGE as u64 - 1) & !(PAGE as u64 - 1) }

/// sbrk as a C library makes it from brk: the old break, or None when brk did not reach
/// the break asked for.
fn sbrk(inc: i64) -> Option<u64> {
    let old = cur_brk();
    let want = (old as i64 + inc) as u64;
    if brk(want) < want { None } else { Some(old) }
}

/// Move the break to `to`, which must succeed.
fn set_brk(to: u64) -> CaseResult {
    let got = brk(to);
    check(got >= to, &format!("brk({to:#x}) left the break at {got:#x}"))
}

fn br_query() -> CaseResult {
    let a = cur_brk();
    let b = cur_brk();
    check(a != 0 && !is_err(a as i64), &format!("brk(0) returned {}", shown(a as i64)))?;
    check(a == b, &format!("brk(0) returned {a:#x} and then {b:#x}"))
}

fn br_grow() -> CaseResult {
    let old = cur_brk();
    let len = 64 * 1024;
    let got = brk(old + len);
    check(got >= old + len, &format!("growing the break by 64 KiB from {old:#x} left it at {got:#x}"))?;
    check(cur_brk() == got, "brk(0) does not report the grown break")?;
    let p = old as *mut u8;
    want_bytes(p, len as usize, 0, "the memory the break grew over")?;
    fill(p, len as usize, 14);
    want_pattern(p, len as usize, 14, "the grown heap after it was written")
}

fn br_grow_exact() -> CaseResult {
    let old = page_up(cur_brk());
    set_brk(old)?;
    let want = old + 100;
    let got = brk(want);
    check(got == want, &format!("brk({want:#x}) returned {got:#x}, not the break asked for"))?;
    check(cur_brk() == want, &format!("brk(0) after brk({want:#x}) returned {:#x}", cur_brk()))
}

fn br_sbrk_sequence() -> CaseResult {
    let Some(a) = sbrk(PAGE as i64) else { return fail("sbrk(4096) failed") };
    let Some(b) = sbrk(PAGE as i64) else { return fail("the second sbrk(4096) failed") };
    check(b == a + PAGE as u64, &format!("sbrk(4096) returned {a:#x} and then {b:#x}, not consecutive blocks"))?;
    let now = sbrk(0).unwrap_or(0);
    check(now == b + PAGE as u64, &format!("sbrk(0) returned {now:#x}, not the end of the second block {:#x}", b + PAGE as u64))?;
    fill_byte(a as *mut u8, PAGE, 0xa1);
    fill_byte(b as *mut u8, PAGE, 0xb2);
    want_bytes(a as *mut u8, PAGE, 0xa1, "the first block")?;
    want_bytes(b as *mut u8, PAGE, 0xb2, "the second block")
}

fn br_shrink() -> CaseResult {
    catch_faults()?;
    let base = page_up(cur_brk());
    let top = base + 4 * PAGE as u64;
    set_brk(top)?;
    fill_byte(base as *mut u8, 4 * PAGE, 0x5c);
    check(sbrk(-2 * PAGE as i64).is_some(), "sbrk(-8192) failed")?;
    let now = cur_brk();
    check(now == top - 2 * PAGE as u64, &format!("after sbrk(-8192) from {top:#x} the break is {now:#x}"))?;
    loads("reading the last byte still below the break", (now - 1) as *mut u8, Some(0x5c))?;
    load_faults("reading the first page above the shrunk break", now as *mut u8, SIGSEGV, Some(SEGV_MAPERR))
}

fn br_regrow_zero() -> CaseResult {
    let base = page_up(cur_brk());
    let top = base + 2 * PAGE as u64;
    set_brk(top)?;
    fill_byte(base as *mut u8, 2 * PAGE, 0xaa);
    let low = brk(base);
    check(low == base, &format!("shrinking the break to {base:#x} left it at {low:#x}"))?;
    set_brk(top)?;
    want_bytes(base as *mut u8, 2 * PAGE, 0, "memory the break regrew over")
}

fn br_below_start() -> CaseResult {
    let old = cur_brk();
    let got = brk(PAGE as u64);
    check(got == old, &format!("brk(0x1000), below the start of the heap, returned {got:#x}, not the unchanged break {old:#x}"))?;
    check(cur_brk() == old, "brk below the start of the heap moved the break")
}

fn br_huge() -> CaseResult {
    let old = cur_brk();
    let want = old + (1u64 << 40);
    let got = brk(want);
    if got >= want { let _ = brk(old); }
    check(got == old, &format!("growing the break by 1 TiB returned {got:#x}, not the unchanged break {old:#x}"))?;
    check(cur_brk() == old, "a refused 1 TiB brk moved the break")
}

fn br_rlimit_data() -> CaseResult {
    // The child allocates nothing while its limit is low.
    let mut child = Child::start(|| {
        let old = page_up(cur_brk());
        if brk(old) < old { return 1; }
        if set_limit(RLIMIT_DATA, 0, RLIM_INFINITY) != 0 { return 2; }
        let got = brk(old + 64 * 1024);
        if got != old { return 3; }
        if set_limit(RLIMIT_DATA, RLIM_INFINITY, RLIM_INFINITY) != 0 { return 4; }
        if brk(old + 64 * 1024) < old + 64 * 1024 { return 5; }
        0
    })?;
    child.finish("the child", &[
        "the child could not align its break",
        "setrlimit(RLIMIT_DATA, 0) failed",
        "with RLIMIT_DATA at 0, growing the break by 64 KiB did not fail and leave it unchanged",
        "raising RLIMIT_DATA again failed",
        "with RLIMIT_DATA raised again, growing the break by 64 KiB failed",
    ])
}

fn br_rlimit_data_mmap() -> CaseResult {
    // The child allocates nothing while its limit is low.
    let mut child = Child::start(|| {
        if set_limit(RLIMIT_DATA, 0, RLIM_INFINITY) != 0 { return 1; }
        if mmap_raw(0, 16 * MIB as u64, RW, PRIVATE, -1, 0) != -ENOMEM { return 2; }
        if is_err(mmap_raw(0, 16 * MIB as u64, PROT_READ, PRIVATE, -1, 0)) { return 3; }
        0
    })?;
    child.finish("the child", &[
        "setrlimit(RLIMIT_DATA, 0) failed",
        "with RLIMIT_DATA at 0, a 16 MiB private writable anonymous mmap did not fail with ENOMEM",
        "with RLIMIT_DATA at 0, a 16 MiB read-only anonymous mmap failed",
    ])
}

fn br_fork_heap() -> CaseResult {
    let base = page_up(cur_brk());
    let top = base + 2 * PAGE as u64;
    set_brk(top)?;
    poke((top - 1) as *mut u8, b'P');
    let mut child = Child::start(|| {
        if cur_brk() != top { return 1; }
        if peek((top - 1) as *mut u8) != b'P' { return 2; }
        let higher = top + 4 * PAGE as u64;
        if brk(higher) < higher { return 3; }
        poke((higher - 1) as *mut u8, b'c');
        0
    })?;
    child.finish("the child", &[
        "the child's break is not the parent's",
        "the child did not see the parent's heap contents",
        "the child could not grow its own break",
    ])?;
    check(cur_brk() == top, &format!("the child's brk moved the parent's break to {:#x}", cur_brk()))?;
    check(peek((top - 1) as *mut u8) == b'P', "the parent's heap byte changed")
}

fn br_heap_large() -> CaseResult {
    let pages = 2048;
    let base = page_up(cur_brk());
    let before = resident()?;
    set_brk(base + (pages * PAGE) as u64)?;
    let p = base as *mut u8;
    let t0 = mono();
    for i in 0..pages { poke(at(p, i * PAGE), i as u8); }
    let took = mono() - t0;
    let grew = resident()? - before;
    value("fault", took / pages as i64, "ns", None);
    value("resident-pages", grew, "", Some((pages as i64, pages as i64 + 64)));
    for i in 0..pages {
        check(peek(at(p, i * PAGE)) == i as u8, &format!("heap page {i} of 2048 did not keep its byte"))?;
    }
    check(grew >= pages as i64, &format!("after touching 2048 heap pages only {grew} more were resident"))
}

// ---------------------------------------------------------------------------
// locking: mlock, msync and advice.

fn lk_mlock() -> CaseResult {
    let pages = 16;
    let p = anon(pages * PAGE)?;
    let before = resident()?;
    zero("mlock of 16 untouched pages", mlock(p, pages * PAGE))?;
    let grew = resident()? - before;
    value("resident-pages", grew, "", Some((pages as i64, pages as i64 + 8)));
    check(grew >= pages as i64, &format!("mlock of 16 untouched pages made only {grew} resident"))?;
    zero("munlock", munlock(p, pages * PAGE))
}

fn lk_munlock() -> CaseResult {
    let p = anon(4 * PAGE)?;
    fill(p, 4 * PAGE, 15);
    zero("mlock", mlock(p, 4 * PAGE))?;
    zero("munlock of the locked range", munlock(p, 4 * PAGE))?;
    want_pattern(p, 4 * PAGE, 15, "the range after mlock and munlock")
}

fn lk_mlock_unaligned() -> CaseResult {
    let p = anon(2 * PAGE)?;
    zero("mlock of 10 bytes at a page address plus 100", mlock(at(p, 100), 10))?;
    zero("munlock of the same 10 bytes", munlock(at(p, 100), 10))
}

fn lk_mlock_enomem() -> CaseResult {
    let p = anon(2 * PAGE)?;
    unmap(at(p, PAGE), PAGE)?;
    want_err("mlock of a mapped page and the unmapped one after it", mlock(p, 2 * PAGE), ENOMEM)
}

fn lk_munlock_enomem() -> CaseResult {
    let p = anon(2 * PAGE)?;
    unmap(at(p, PAGE), PAGE)?;
    want_err("munlock of a mapped page and the unmapped one after it", munlock(p, 2 * PAGE), ENOMEM)
}

fn lk_mlock_eperm() -> CaseResult {
    let p = anon(PAGE)?;
    let mut child = Child::start(|| {
        if set_limit(RLIMIT_MEMLOCK, 0, 0) != 0 { return 1; }
        if sc(nr::SETUID, &[USER_A as u64]) != 0 { return 2; }
        if mlock(p, PAGE) != -EPERM { return 3; }
        0
    })?;
    child.finish("the child", &[
        "setrlimit(RLIMIT_MEMLOCK, 0) failed",
        "setuid failed",
        "an unprivileged process with RLIMIT_MEMLOCK at 0 was not refused mlock with EPERM",
    ])
}

fn lk_mlock_limit() -> CaseResult {
    let p = anon(32 * PAGE)?;
    let mut child = Child::start(|| {
        if set_limit(RLIMIT_MEMLOCK, 64 * 1024, 64 * 1024) != 0 { return 1; }
        if sc(nr::SETUID, &[USER_A as u64]) != 0 { return 2; }
        if mlock(p, 8 * PAGE) != 0 { return 3; }
        if mlock(at(p, 8 * PAGE), 16 * PAGE) != -ENOMEM { return 4; }
        0
    })?;
    child.finish("the child", &[
        "setrlimit(RLIMIT_MEMLOCK, 64 KiB) failed",
        "setuid failed",
        "an unprivileged process could not mlock 32 KiB under a 64 KiB RLIMIT_MEMLOCK",
        "an unprivileged process locking 96 KiB under a 64 KiB RLIMIT_MEMLOCK was not refused with ENOMEM",
    ])
}

fn lk_mlockall_current() -> CaseResult {
    let pages = 16;
    let p = anon(pages * PAGE)?;
    let before = resident()?;
    zero("mlockall(MCL_CURRENT)", mlockall(MCL_CURRENT))?;
    let grew = resident()? - before;
    let _ = munlockall();
    value("resident-pages", grew, "", Some((pages as i64, i64::MAX / 2)));
    check(grew >= pages as i64, &format!("after mlockall(MCL_CURRENT) an untouched 16-page mapping made only {grew} pages resident"))?;
    unmap(p, pages * PAGE)
}

fn lk_mlockall_future() -> CaseResult {
    let pages = 16;
    zero("mlockall(MCL_FUTURE)", mlockall(MCL_FUTURE))?;
    let before = resident()?;
    let p = anon(pages * PAGE)?;
    let grew = resident()? - before;
    let _ = munlockall();
    value("resident-pages", grew, "", Some((pages as i64, pages as i64 + 16)));
    check(grew >= pages as i64, &format!("after mlockall(MCL_FUTURE) a new 16-page mapping made only {grew} pages resident"))?;
    unmap(p, pages * PAGE)
}

fn lk_mlockall_einval() -> CaseResult {
    want_err("mlockall(0)", mlockall(0), EINVAL)?;
    want_err("mlockall with unknown flag 8", mlockall(8), EINVAL)
}

fn lk_munlockall() -> CaseResult {
    zero("munlockall with nothing locked", munlockall())?;
    let p = anon(4 * PAGE)?;
    fill(p, 4 * PAGE, 16);
    zero("mlock", mlock(p, 4 * PAGE))?;
    zero("munlockall after mlock", munlockall())?;
    want_pattern(p, 4 * PAGE, 16, "the range after munlockall")
}

fn lk_msync_async() -> CaseResult {
    let f = TempFile::new("msync-async", &[0u8; PAGE])?;
    let p = map(0, PAGE, RW, MAP_SHARED, f.fd)?;
    fill(p, PAGE, 17);
    zero("msync(MS_ASYNC)", msync(p, PAGE, MS_ASYNC))?;
    let back = pread_n(f.fd, PAGE, 0)?;
    check((0..PAGE).all(|off| back.get(off) == Some(&pat(off, 17))), "after msync(MS_ASYNC), read() did not return the stores made through the mapping")
}

fn lk_msync_invalidate() -> CaseResult {
    let f = TempFile::new("msync-invalidate", &[b'o'; PAGE])?;
    let p = map(0, PAGE, RW, MAP_SHARED, f.fd)?;
    check(peek(p) == b'o', "the mapping does not show the file")?;
    pwrite_all(f.fd, b"new", 0)?;
    zero("msync(MS_INVALIDATE)", msync(p, PAGE, MS_INVALIDATE))?;
    let seen: Vec<u8> = (0..3).map(|i| peek(at(p, i))).collect();
    check(seen == b"new", "after write() and msync(MS_INVALIDATE), the mapping does not show the file's new bytes")
}

fn lk_msync_both() -> CaseResult {
    let p = anon(PAGE)?;
    want_err("msync(MS_SYNC|MS_ASYNC)", msync(p, PAGE, MS_SYNC | MS_ASYNC), EINVAL)
}

fn lk_msync_flags() -> CaseResult {
    let p = anon(PAGE)?;
    want_err("msync with unknown flag 8", msync(p, PAGE, 8), EINVAL)
}

fn lk_msync_unaligned() -> CaseResult {
    let p = anon(PAGE)?;
    want_err("msync at a page address plus 1", msync(at(p, 1), PAGE, MS_SYNC), EINVAL)
}

fn lk_msync_unmapped() -> CaseResult {
    let f = TempFile::new("msync-unmapped", &[0u8; 2 * PAGE])?;
    let p = map(0, 2 * PAGE, RW, MAP_SHARED, f.fd)?;
    unmap(at(p, PAGE), PAGE)?;
    want_err("msync of a mapped page and the unmapped one after it", msync(p, 2 * PAGE, MS_SYNC), ENOMEM)
}

fn lk_msync_locked() -> CaseResult {
    let f = TempFile::new("msync-locked", &[0u8; PAGE])?;
    let p = map(0, PAGE, RW, MAP_SHARED, f.fd)?;
    zero("mlock", mlock(p, PAGE))?;
    want_err("msync(MS_INVALIDATE) of a locked range", msync(p, PAGE, MS_INVALIDATE), EBUSY)
}

fn lk_msync_anon() -> CaseResult {
    let p = anon(PAGE)?;
    poke(p, 1);
    zero("msync(MS_SYNC) of an anonymous mapping", msync(p, PAGE, MS_SYNC))
}

fn lk_posix_madvise() -> CaseResult {
    let p = anon(4 * PAGE)?;
    fill(p, 4 * PAGE, 18);
    for (advice, name) in [(POSIX_MADV_NORMAL, "NORMAL"), (POSIX_MADV_SEQUENTIAL, "SEQUENTIAL"), (POSIX_MADV_RANDOM, "RANDOM"), (POSIX_MADV_WILLNEED, "WILLNEED")] {
        zero(&format!("posix_madvise(POSIX_MADV_{name})"), madvise(p, 4 * PAGE, advice))?;
        want_pattern(p, 4 * PAGE, 18, &format!("the range after POSIX_MADV_{name}"))?;
    }
    Ok(())
}

fn lk_posix_madvise_einval() -> CaseResult {
    let p = anon(PAGE)?;
    want_err("posix_madvise with advice 999", madvise(p, PAGE, 999), EINVAL)
}

fn lk_posix_madvise_enomem() -> CaseResult {
    let p = anon(2 * PAGE)?;
    unmap(at(p, PAGE), PAGE)?;
    want_err("posix_madvise of a mapped page and the unmapped one after it", madvise(p, 2 * PAGE, POSIX_MADV_NORMAL), ENOMEM)
}

fn lk_madvise_unaligned() -> CaseResult {
    let p = anon(2 * PAGE)?;
    want_err("posix_madvise at a page address plus 1", madvise(at(p, 1), PAGE, POSIX_MADV_NORMAL), EINVAL)
}

fn lk_madvise_dontneed() -> CaseResult {
    let pages = 8;
    let p = anon(pages * PAGE)?;
    fill_byte(p, pages * PAGE, 0x5a);
    let before = resident()?;
    zero("madvise(MADV_DONTNEED)", madvise(p, pages * PAGE, MADV_DONTNEED))?;
    let freed = before - resident()?;
    value("freed-pages", freed, "", Some((pages as i64, pages as i64 + 8)));
    want_bytes(p, pages * PAGE, 0, "private anonymous memory after MADV_DONTNEED")?;
    check(freed >= pages as i64, &format!("MADV_DONTNEED of 8 resident pages freed {freed}"))
}

fn lk_mincore() -> CaseResult {
    let pages = 8;
    let p = anon(pages * PAGE)?;
    for i in [0, 2, 4] { poke(at(p, i * PAGE), 1); }
    let mut vec = [0xffu8; 8];
    zero("mincore", mincore(p, pages * PAGE, &mut vec))?;
    let resident: Vec<usize> = (0..pages).filter(|&i| vec[i] & 1 == 1).collect();
    value("resident-pages", resident.len() as i64, "", Some((3, 3)));
    check(resident == [0, 2, 4], &format!("mincore reports pages {resident:?} resident; 0, 2 and 4 were touched"))
}

fn lk_mincore_enomem() -> CaseResult {
    let p = anon(2 * PAGE)?;
    unmap(at(p, PAGE), PAGE)?;
    let mut vec = [0u8; 2];
    want_err("mincore of a mapped page and the unmapped one after it", mincore(p, 2 * PAGE, &mut vec), ENOMEM)
}

fn lk_mincore_einval() -> CaseResult {
    let p = anon(2 * PAGE)?;
    let mut vec = [0u8; 2];
    want_err("mincore at a page address plus 1", mincore(at(p, 1), PAGE, &mut vec), EINVAL)
}

static SUITE: Suite = suite(
    "memory", "Memory", &[
        category("anonymous", "anonymous mmap & munmap", &[
            case("private-zero", "An anonymous MAP_PRIVATE mapping of 16 pages reads as zero throughout and keeps what is written to every page", an_private_zero),
            case("shared-zero", "An anonymous MAP_SHARED mapping of 16 pages reads as zero throughout and keeps what is written to every page", an_shared_zero),
            case("page-aligned", "mmap returns a page-aligned address for lengths of 1, 4095, 4096 and 4097 bytes", an_page_aligned),
            case("partial-page", "A mapping whose length is not a multiple of the page size covers the whole last page: its last byte reads zero and can be written", an_partial_page),
            case("distinct", "Two anonymous mappings made one after the other do not overlap, and writing one leaves the other unchanged", an_distinct),
            case("hint-occupied", "mmap without MAP_FIXED, given a hint inside an existing mapping, places the new mapping elsewhere and leaves the existing one unchanged", an_hint_occupied),
            case("hint-free", "Linux policy: mmap without MAP_FIXED uses a free, page-aligned hint address exactly", an_hint_free),
            case("fixed-replace", "MAP_FIXED over the middle page of a 3-page mapping replaces it with a zero-filled page at exactly that address and leaves the pages either side unchanged", an_fixed_replace),
            case("fixed-unaligned", "MAP_FIXED at an address that is not page-aligned fails with EINVAL and leaves the mapping there unchanged", an_fixed_unaligned),
            case("noreplace-busy", "Linux ABI: MAP_FIXED_NOREPLACE at an address already mapped fails with EEXIST and leaves that mapping unchanged", an_noreplace_busy),
            case("noreplace-free", "Linux ABI: MAP_FIXED_NOREPLACE at a free page-aligned address maps exactly there", an_noreplace_free),
            case("length-zero", "mmap of length 0 fails with EINVAL", an_length_zero),
            case("no-type", "mmap with neither MAP_SHARED nor MAP_PRIVATE fails with EINVAL", an_no_type),
            case("offset-unaligned", "mmap with an offset that is not a multiple of the page size fails with EINVAL", an_offset_unaligned),
            case("ebadf", "mmap without MAP_ANONYMOUS of descriptor -1 or of a closed descriptor fails with EBADF", an_ebadf),
            case("enomem-space", "mmap of 2^60 bytes, more address space than a process has, fails with ENOMEM", an_enomem_space),
            case("enomem-fixed", "MAP_FIXED at 2^56, past the end of the user address space, fails with ENOMEM", an_enomem_fixed),
            case("rlimit-as", "With RLIMIT_AS at 256 MiB, a 512 MiB mmap that succeeds without the limit fails with ENOMEM, and a 16 MiB one succeeds", an_rlimit_as),
            case("munmap-whole", "After munmap, every page of the former mapping raises SIGSEGV with SEGV_MAPERR and si_addr the byte touched", an_munmap_whole),
            case("munmap-middle", "munmap of the middle page of a 3-page mapping splits it: the outer pages keep their contents and the middle one raises SEGV_MAPERR", an_munmap_middle),
            case("munmap-head-tail", "munmap of the first and last pages of a 4-page mapping leaves the middle two with their contents and the ends unmapped", an_munmap_head_tail),
            case("munmap-unmapped", "munmap of a page-aligned range with nothing mapped in it succeeds", an_munmap_unmapped),
            case("munmap-span", "One munmap across two separate mappings and the gap between them removes both", an_munmap_span),
            case("munmap-unaligned", "munmap at an address that is not page-aligned fails with EINVAL and leaves the mapping in place", an_munmap_unaligned),
            case("munmap-zero", "munmap of length 0 fails with EINVAL", an_munmap_zero),
            case("remap-zero", "A page mapped with MAP_FIXED into the hole a munmap left reads as zero, not the old contents", an_remap_zero),
            case("sparse", "Linux policy: touching one byte every 1 MiB of a 256 MiB anonymous mapping makes about 256 pages resident, not 65536", an_sparse),
            case("page-by-page", "Each page of a 16 MiB anonymous mapping, touched in order, keeps its own contents, and all 4096 are resident afterwards", an_page_by_page),
            case("churn", "Mapping, touching and unmapping 1 MiB 100 times leaves free memory within 4 MiB of where it was and the resident set no larger", an_churn),
            case("many", "1000 one-page anonymous mappings exist at once at distinct addresses, each keeping its own contents, and all unmap", an_many),
            case("unmap-cpus", "With two to four processors, a thread reading a page another thread has just unmapped raises SEGV_MAPERR every time and never reads the old page", an_unmap_cpus),
        ]),
        category("protection", "mprotect & faults", &[
            case("none-read", "Reading a PROT_NONE mapping raises SIGSEGV with SEGV_ACCERR and si_addr the byte read", pr_none_read),
            case("none-write", "Writing a PROT_NONE mapping raises SIGSEGV with SEGV_ACCERR and si_addr the byte written", pr_none_write),
            case("readonly-write", "Writing a PROT_READ mapping raises SIGSEGV with SEGV_ACCERR and leaves the byte unchanged, while reading succeeds", pr_readonly_write),
            case("unmapped-maperr", "Reading an address with nothing mapped raises SIGSEGV with SEGV_MAPERR and si_addr that address", pr_unmapped_maperr),
            case("handler-retry", "A SIGSEGV handler that mprotects the faulting page writable and returns lets the write complete: 64 PROT_NONE pages take exactly 64 faults", pr_handler_retry),
            case("make-readonly", "mprotect to PROT_READ keeps the contents readable and makes writes raise SEGV_ACCERR", pr_make_readonly),
            case("make-writable", "mprotect from PROT_READ back to PROT_READ|PROT_WRITE lets writes succeed and keeps the contents", pr_make_writable),
            case("none-keeps", "mprotect to PROT_NONE and back to PROT_READ|PROT_WRITE keeps the contents, and reads fault in between", pr_none_keeps),
            case("guard-page", "A PROT_NONE guard page in the middle of a mapping faults at its first and last bytes while the bytes either side stay accessible", pr_guard_page),
            case("partial", "mprotect of the first page of a 4-page mapping to PROT_READ changes only that page", pr_partial),
            case("partial-middle", "mprotect of two middle pages of a 6-page mapping to PROT_READ and of one of them back leaves exactly the other read-only", pr_partial_middle),
            case("unaligned", "mprotect at an address that is not page-aligned fails with EINVAL and leaves the protection unchanged", pr_unaligned),
            case("unmapped-enomem", "mprotect of a range that includes an unmapped page fails with ENOMEM", pr_unmapped_enomem),
            case("bad-prot", "Linux ABI: mprotect with an unknown protection bit fails with EINVAL", pr_bad_prot),
            case("file-eacces", "mprotect adding PROT_WRITE to a MAP_SHARED mapping of a file opened read-only fails with EACCES", pr_file_eacces),
            case("exec-denied", "Calling into a PROT_READ|PROT_WRITE anonymous mapping raises SIGSEGV with SEGV_ACCERR at the call's target (ARM64 and x86-64 both enforce no-execute)", pr_exec_denied),
            case("exec-file", "Code in a file mapped PROT_READ|PROT_EXEC runs, and after mprotect to PROT_READ calling it raises SIGSEGV with SEGV_ACCERR", pr_exec_file),
            case("exec-add", "Calling code in a file mapped PROT_READ raises SIGSEGV, and after mprotect adds PROT_EXEC the code runs", pr_exec_add),
            case("exec-jit", "Linux ABI: code written into an anonymous mapping and made coherent as a C library's __clear_cache does (ARM64 DC CVAU and IC IVAU from user mode) runs after mprotect to PROT_READ|PROT_EXEC", pr_exec_jit),
            case("fork-inherits", "A forked child inherits each page's protection: its PROT_NONE page faults and its PROT_READ page refuses writes but reads", pr_fork_inherits),
            case("tlb-cpus", "With two to four processors, a write by a thread on another processor begun after mprotect to PROT_READ returned always raises SEGV_ACCERR", pr_tlb_cpus),
        ]),
        category("shared", "shared mappings across fork", &[
            case("anon-child-sees", "A MAP_SHARED anonymous page written by the parent after fork is seen by the child", sh_anon_child_sees),
            case("anon-parent-sees", "A MAP_SHARED anonymous page written by the child is seen by the parent", sh_anon_parent_sees),
            case("pingpong", "Parent and child take 100 turns storing to a MAP_SHARED anonymous page, each seeing the other's latest store", sh_pingpong),
            case("anon-atomic", "Two processes each atomically adding 1 to a counter in a MAP_SHARED anonymous page 100000 times leave 200000", sh_anon_atomic),
            case("private-child-isolated", "After fork, a child's stores to a MAP_PRIVATE anonymous page are not seen by the parent", sh_private_child_isolated),
            case("private-parent-isolated", "After fork, the parent's stores to a MAP_PRIVATE anonymous page are not seen by the child", sh_private_parent_isolated),
            case("private-cow-many", "After fork, parent and child each rewrite all 64 pages of a MAP_PRIVATE mapping and each sees only its own stores", sh_private_cow_many),
            case("file-child-sees", "A child's store into a MAP_SHARED file mapping inherited across fork is seen through the parent's mapping and by read()", sh_file_child_sees),
            case("file-independent", "Two processes that each map the same file MAP_SHARED see each other's stores", sh_file_independent),
            case("file-msync-read", "Stores into a MAP_SHARED file mapping are what read() returns from the file after msync(MS_SYNC)", sh_file_msync_read),
            case("file-write-seen", "A write() to a file is seen at once through an existing MAP_SHARED mapping of it", sh_file_write_seen),
            case("file-private", "Stores into a MAP_PRIVATE file mapping are not written to the file and not seen by another process's MAP_SHARED mapping of it", sh_file_private),
            case("child-unmaps", "A child's munmap of an inherited MAP_SHARED anonymous mapping leaves the parent's view, and its later stores, in place", sh_child_unmaps),
            case("parent-unmaps", "After the parent unmaps a MAP_SHARED anonymous mapping, the child still reads the parent's last store and can store its own", sh_parent_unmaps),
            case("eof-zero-tail", "The part of a file mapping's last page past the end of the file reads as zero, and stores there are not written to the file", sh_eof_zero_tail),
            case("eof-sigbus", "Touching a page of a file mapping that lies wholly past the end of the file raises SIGBUS with si_addr the byte touched", sh_eof_sigbus),
            case("eof-truncated", "After the file is truncated, touching a page of its MAP_SHARED mapping past the new end raises SIGBUS", sh_eof_truncated),
            case("write-only-fd", "mmap of a file opened O_WRONLY fails with EACCES", sh_write_only_fd),
            case("readonly-shared-write", "mmap with PROT_WRITE and MAP_SHARED of a file opened O_RDONLY fails with EACCES", sh_readonly_shared_write),
            case("readonly-private-write", "mmap with PROT_WRITE and MAP_PRIVATE of a file opened O_RDONLY succeeds, and its stores stay private", sh_readonly_private_write),
            case("pipe-enodev", "mmap of a pipe fails with ENODEV", sh_pipe_enodev),
        ]),
        category("brk", "brk & sbrk", &[
            case("query", "brk(0), as a C library's sbrk(0) makes it, reports the same nonzero break each time", br_query),
            case("grow", "Growing the break by 64 KiB succeeds, and the new memory reads as zero and keeps what is written", br_grow),
            case("grow-exact", "Linux ABI: brk sets and returns exactly the break asked for, not one rounded up to a page", br_grow_exact),
            case("sbrk-sequence", "Successive sbrk(4096) calls return consecutive blocks, each starting at the previous break", br_sbrk_sequence),
            case("shrink", "Shrinking the break with sbrk(-8192) moves it down, and touching memory above the new break raises SIGSEGV with SEGV_MAPERR", br_shrink),
            case("regrow-zero", "Memory given back by shrinking the break and then regrown reads as zero, not the old contents", br_regrow_zero),
            case("below-start", "brk below the start of the heap fails and leaves the break unchanged", br_below_start),
            case("huge", "Growing the break by 1 TiB fails and leaves the break unchanged", br_huge),
            case("rlimit-data", "With RLIMIT_DATA at 0, growing the break fails and leaves it unchanged; with the limit raised again the same growth succeeds", br_rlimit_data),
            case("rlimit-data-mmap", "Linux policy: with RLIMIT_DATA at 0, a private writable anonymous mmap fails with ENOMEM while a read-only one succeeds", br_rlimit_data_mmap),
            case("fork-heap", "A forked child inherits the break and the heap's contents, and its brk leaves the parent's break unchanged", br_fork_heap),
            case("heap-large", "Growing the break by 8 MiB gives 2048 pages that each keep their contents and are all resident afterwards", br_heap_large),
        ]),
        category("locking", "mlock & msync", &[
            case("mlock", "mlock of 16 untouched anonymous pages succeeds and makes them resident", lk_mlock),
            case("munlock", "munlock of a locked range succeeds and leaves its contents unchanged", lk_munlock),
            case("mlock-unaligned", "Linux policy: mlock and munlock at an address inside a page lock and unlock the whole pages and succeed", lk_mlock_unaligned),
            case("mlock-enomem", "mlock of a range that includes an unmapped page fails with ENOMEM", lk_mlock_enomem),
            case("munlock-enomem", "munlock of a range that includes an unmapped page fails with ENOMEM", lk_munlock_enomem),
            case("mlock-eperm", "Linux policy: an unprivileged process with RLIMIT_MEMLOCK at 0 is refused mlock with EPERM", lk_mlock_eperm),
            case("mlock-limit", "Linux policy: an unprivileged process may lock 32 KiB under a 64 KiB RLIMIT_MEMLOCK and is refused 64 KiB more with ENOMEM", lk_mlock_limit),
            case("mlockall-current", "mlockall(MCL_CURRENT) succeeds and makes an untouched mapping that existed before it resident", lk_mlockall_current),
            case("mlockall-future", "After mlockall(MCL_FUTURE), a new anonymous mapping is resident as soon as mmap returns", lk_mlockall_future),
            case("mlockall-einval", "mlockall with no flags or with an unknown flag fails with EINVAL", lk_mlockall_einval),
            case("munlockall", "munlockall succeeds whether or not anything is locked, and leaves contents unchanged", lk_munlockall),
            case("msync-async", "msync(MS_ASYNC) of a MAP_SHARED file mapping returns 0 and read() returns the stores made through it", lk_msync_async),
            case("msync-invalidate", "msync(MS_INVALIDATE) of a MAP_SHARED file mapping returns 0, and the mapping shows a write() made to the file", lk_msync_invalidate),
            case("msync-both", "msync with both MS_SYNC and MS_ASYNC fails with EINVAL", lk_msync_both),
            case("msync-flags", "msync with an unknown flag fails with EINVAL", lk_msync_flags),
            case("msync-unaligned", "msync at an address that is not page-aligned fails with EINVAL", lk_msync_unaligned),
            case("msync-unmapped", "msync of a range that includes an unmapped page fails with ENOMEM", lk_msync_unmapped),
            case("msync-locked", "msync(MS_INVALIDATE) of a range locked by mlock fails with EBUSY", lk_msync_locked),
            case("msync-anon", "Linux policy: msync(MS_SYNC) of an anonymous mapping returns 0", lk_msync_anon),
            case("posix-madvise", "posix_madvise with POSIX_MADV_NORMAL, SEQUENTIAL, RANDOM and WILLNEED returns 0 and leaves the contents unchanged", lk_posix_madvise),
            case("posix-madvise-einval", "posix_madvise with an unknown advice value returns EINVAL", lk_posix_madvise_einval),
            case("posix-madvise-enomem", "posix_madvise of a range that includes an unmapped page returns ENOMEM", lk_posix_madvise_enomem),
            case("madvise-unaligned", "Linux policy: posix_madvise at an address that is not page-aligned returns EINVAL", lk_madvise_unaligned),
            case("madvise-dontneed", "Linux ABI: madvise(MADV_DONTNEED) of private anonymous memory frees its pages, which then read as zero", lk_madvise_dontneed),
            case("mincore", "Linux ABI: mincore reports the touched pages of an anonymous mapping resident and the untouched ones not", lk_mincore),
            case("mincore-enomem", "Linux ABI: mincore of a range that includes an unmapped page fails with ENOMEM", lk_mincore_enomem),
            case("mincore-einval", "Linux ABI: mincore at an address that is not page-aligned fails with EINVAL", lk_mincore_einval),
        ]),
    ],
);

fn main() { SUITE.run() }
