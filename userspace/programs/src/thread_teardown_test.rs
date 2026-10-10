//! A thread outlives its process's first thread (#1321).
//!
//! A child process starts one thread with CLONE_CHILD_CLEARTID and parks it in a
//! 300 ms FUTEX_WAIT, so no CPU has the child's address space loaded. The child's
//! first thread then ends with exit (not exit_group): the process lives on in the
//! parked thread. The parent maps and fills fresh pages, which takes any frames the
//! exit freed, and waits. The parked thread must wake on the address space it was
//! started on, write its result to a page shared with the parent and exit, and its
//! exit-cleared thread-ID word must land in its own memory, not in the parent's
//! pages. The parent reaps the child only once both threads have ended.
//!
//! The interleaving is checked, not assumed: the wait must end in ETIMEDOUT after
//! the full 300 ms (the thread stayed parked), the first thread must be gone when
//! the parked one wakes (tkill of it gives ESRCH), and the parent's fill must have
//! finished before the wake (CLOCK_MONOTONIC times on the shared page).
//!
//! Prints `THREAD_TEARDOWN PASS` or `THREAD_TEARDOWN FAIL <reason>`.

use libbreenix::syscall::{nr, raw};

const CLONE_VM: u64 = 0x100;
const CLONE_FS: u64 = 0x200;
const CLONE_FILES: u64 = 0x400;
const CLONE_SIGHAND: u64 = 0x800;
const CLONE_THREAD: u64 = 0x10000;
const CLONE_CHILD_CLEARTID: u64 = 0x200000;
const CLONE_CHILD_SETTID: u64 = 0x1000000;

const PROT_RW: u64 = 3;
const MAP_SHARED_ANON: u64 = 0x21;
const MAP_PRIVATE_ANON: u64 = 0x22;
const WNOHANG: u64 = 1;
const FUTEX_WAIT: u64 = 0;
#[cfg(target_arch = "x86_64")]
const TKILL: u64 = 200;
#[cfg(target_arch = "aarch64")]
const TKILL: u64 = 130;

const PAGE: usize = 4096;
const FILL_PAGES: usize = 64;
const PATTERN: u8 = 0xa5;
const TID_OFFSET: usize = 24;

/// Words of the page shared between the parent and the child.
const STARTED: usize = 0;
/// The parked thread's result: `ALIVE`, or one of the `WOKE_*` codes.
const RESULT: usize = 1;
/// CLOCK_MONOTONIC nanoseconds just before the first thread's exit.
const MAIN_EXITED: usize = 2;
/// The first thread's thread ID.
const LEADER_TID: usize = 3;
/// CLOCK_MONOTONIC nanoseconds: just before the wait, and just after it.
const PARKED_AT: usize = 4;
const WOKE_AT: usize = 5;
/// What the futex wait returned.
const WAIT_RETURN: usize = 6;
const ALIVE: u64 = 0x600d;
/// The wait did not end in ETIMEDOUT after its full 300 ms.
const WOKE_EARLY: u64 = 1;
/// The first thread had not exited when the parked thread woke.
const WOKE_LEADER_LIVE: u64 = 2;
const PARK_NS: i64 = 300_000_000;
const ETIMEDOUT: i64 = 110;
const ESRCH: i64 = 3;

fn sc(n: u64, a: [u64; 6]) -> i64 {
    // SAFETY: raw system calls with arguments that are plain values or pointers
    // to memory this program owns.
    unsafe { raw::syscall6(n, a[0], a[1], a[2], a[3], a[4], a[5]) as i64 }
}

fn mmap(len: usize, flags: u64) -> *mut u8 {
    let r = sc(nr::MMAP, [0, len as u64, PROT_RW, flags, u64::MAX, 0]);
    if r < 0 { core::ptr::null_mut() } else { r as *mut u8 }
}

fn word(page: *mut u8, i: usize) -> &'static core::sync::atomic::AtomicU64 {
    // SAFETY: `page` is a live, page-aligned mapping that is never unmapped.
    unsafe { &*(page as *const core::sync::atomic::AtomicU64).add(i) }
}

fn monotonic_ns() -> u64 {
    let mut ts = [0i64; 2];
    let _ = sc(nr::CLOCK_GETTIME, [1, ts.as_mut_ptr() as u64, 0, 0, 0, 0]);
    (ts[0] * 1_000_000_000 + ts[1]) as u64
}

fn sleep_ms(ms: u64) {
    let ts = [0i64, (ms * 1_000_000) as i64];
    let _ = sc(nr::NANOSLEEP, [ts.as_ptr() as u64, 0, 0, 0, 0, 0]);
}

/// The parked thread: `arg` is the shared page. Its stack holds the futex word.
extern "C" fn parked(arg: u64) -> ! {
    let shared = arg as *mut u8;
    let seq = core::sync::atomic::Ordering::SeqCst;
    word(shared, STARTED).store(1, seq);
    let futex = 7u32;
    let timeout = [0i64, PARK_NS];
    let parked_at = monotonic_ns();
    word(shared, PARKED_AT).store(parked_at, seq);
    let waited = sc(nr::FUTEX, [&futex as *const u32 as u64, FUTEX_WAIT, 7, timeout.as_ptr() as u64, 0, 0]);
    let woke_at = monotonic_ns();
    // Signal 0 checks that the first thread still exists, sending nothing.
    let leader = sc(TKILL, [word(shared, LEADER_TID).load(seq), 0, 0, 0, 0, 0]);
    word(shared, WOKE_AT).store(woke_at, seq);
    word(shared, WAIT_RETURN).store(waited as u64, seq);
    let result = if waited != -ETIMEDOUT || woke_at.saturating_sub(parked_at) < PARK_NS as u64 {
        WOKE_EARLY
    } else if leader != -ESRCH {
        WOKE_LEADER_LIVE
    } else {
        ALIVE
    };
    word(shared, RESULT).store(result, seq);
    let _ = sc(nr::EXIT, [0; 6]);
    loop {
        core::hint::spin_loop();
    }
}

fn child(shared: *mut u8) -> ! {
    let info = mmap(PAGE, MAP_PRIVATE_ANON);
    let stack = mmap(16 * PAGE, MAP_PRIVATE_ANON);
    if info.is_null() || stack.is_null() {
        let _ = sc(nr::EXIT_GROUP, [2, 0, 0, 0, 0, 0]);
    }
    word(shared, LEADER_TID).store(sc(nr::GETTID, [0; 6]) as u64, core::sync::atomic::Ordering::SeqCst);
    let tid_word = unsafe { info.add(TID_OFFSET) } as u64;
    let stack_top = (stack as u64 + 16 * PAGE as u64) & !0xf;
    let flags = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD
        | CLONE_CHILD_CLEARTID | CLONE_CHILD_SETTID;
    let r = sc(nr::CLONE, [flags, stack_top, parked as usize as u64, shared as u64, tid_word, 0]);
    if r < 0 {
        let _ = sc(nr::EXIT_GROUP, [3, 0, 0, 0, 0, 0]);
    }
    while word(shared, STARTED).load(core::sync::atomic::Ordering::SeqCst) == 0 {
        sleep_ms(1);
    }
    // Let the thread reach its futex wait, then end this thread only.
    sleep_ms(50);
    word(shared, MAIN_EXITED).store(monotonic_ns(), core::sync::atomic::Ordering::SeqCst);
    let _ = sc(nr::EXIT, [0; 6]);
    loop {
        core::hint::spin_loop();
    }
}

fn fail(reason: &str) -> ! {
    println!("THREAD_TEARDOWN FAIL {reason}");
    std::process::exit(1);
}

fn main() {
    let shared = mmap(PAGE, MAP_SHARED_ANON);
    if shared.is_null() {
        fail("mmap of the shared page failed");
    }
    let pid = match libbreenix::process::fork() {
        Ok(libbreenix::process::ForkResult::Child) => child(shared),
        Ok(libbreenix::process::ForkResult::Parent(pid)) => pid.raw() as i64,
        Err(_) => fail("fork failed"),
    };
    let mut waited = 0;
    while word(shared, MAIN_EXITED).load(core::sync::atomic::Ordering::SeqCst) == 0 {
        if waited > 2000 {
            fail("the child's first thread did not exit within 2 s");
        }
        sleep_ms(1);
        waited += 1;
    }
    sleep_ms(20);
    // Take whatever frames the first thread's exit freed.
    let len = FILL_PAGES * PAGE;
    let fill = mmap(len, MAP_PRIVATE_ANON);
    if fill.is_null() {
        fail("mmap of the fill pages failed");
    }
    unsafe { core::ptr::write_bytes(fill, PATTERN, len) };
    let filled_at = monotonic_ns();
    // The parked thread wakes 300 ms after it parked; give it time to finish.
    let mut status = 0i32;
    let mut reaped = false;
    for _ in 0..1000 {
        let r = sc(nr::WAIT4, [pid as u64, &mut status as *mut i32 as u64, WNOHANG, 0, 0, 0]);
        if r == pid {
            reaped = true;
            break;
        }
        sleep_ms(2);
    }
    let bytes = unsafe { core::slice::from_raw_parts(fill, len) };
    let changed = bytes.iter().filter(|&&b| b != PATTERN).count();
    let seq = core::sync::atomic::Ordering::SeqCst;
    let result = word(shared, RESULT).load(seq);
    let woke_at = word(shared, WOKE_AT).load(seq);
    if !reaped {
        fail("the child was not reaped within 2 s");
    }
    match result {
        ALIVE => {}
        WOKE_EARLY => fail(&format!(
            "the parked thread's 300 ms wait returned {} after {} ms, so it was not parked throughout",
            word(shared, WAIT_RETURN).load(seq) as i64,
            woke_at.saturating_sub(word(shared, PARKED_AT).load(seq)) / 1_000_000
        )),
        WOKE_LEADER_LIVE => fail("the first thread had not exited when the parked thread woke"),
        _ => fail(&format!("the parked thread did not run after the first thread exited (wait status {status:#x})")),
    }
    // The wait began no later than its full length before it ended: the first
    // thread's exit must have started after that, while the thread was parked.
    let exit_started = word(shared, MAIN_EXITED).load(seq);
    if exit_started <= woke_at.saturating_sub(PARK_NS as u64) {
        fail("the first thread's exit began before the other thread parked");
    }
    if filled_at >= woke_at {
        fail("the parent's fill finished after the parked thread woke, so it did not take the freed frames first");
    }
    if changed != 0 {
        let at = bytes.iter().position(|&b| b != PATTERN).unwrap_or(0);
        fail(&format!("{changed} bytes of the parent's fresh pages changed, the first at page offset {}", at % PAGE));
    }
    if status != 0 {
        fail(&format!("the child ended with wait status {status:#x}, expected exit 0"));
    }
    println!("THREAD_TEARDOWN PASS");
}
