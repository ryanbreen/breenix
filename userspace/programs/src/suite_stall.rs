//! Scratch stall measurement (never merged). Before a one-case suite, it runs thirty
//! 300 ms burns on one thread, then one burner pinned to each CPU for 30 s, and prints
//! every gap of 4 ms or more between consecutive monotonic clock reads at once, with
//! how much of the gap the thread's CPU-time clock charged.
#[cfg(target_arch = "aarch64")]
use libbreenix::syscall::raw;
use libbreenix::process::{self, ForkResult};
use libbreenix::suite::{case, category, suite, CaseResult, Suite};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const WRITE: u64 = 1;
    pub const SCHED_SETAFFINITY: u64 = 203;
    pub const CLOCK_GETTIME: u64 = 228;
    pub const GETCPU: u64 = 309;
}
#[cfg(target_arch = "aarch64")]
mod nr {
    pub const WRITE: u64 = 64;
    pub const SCHED_SETAFFINITY: u64 = 122;
    pub const CLOCK_GETTIME: u64 = 113;
    pub const GETCPU: u64 = 168;
}

const CLOCK_REALTIME: i32 = 0;
const CLOCK_MONOTONIC: i32 = 1;
const CLOCK_PROCESS_CPUTIME_ID: i32 = 2;
const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
const GAP_NS: i64 = 4_000_000;
const PINNED_NS: i64 = 30_000_000_000;

fn sc(n: u64, args: &[u64]) -> i64 {
    let a = |i: usize| args.get(i).copied().unwrap_or(0);
    #[cfg(target_arch = "x86_64")]
    {
        let ret: i64;
        // SAFETY: callers keep their buffers alive through the call.
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
    // SAFETY: callers keep their buffers alive through the call.
    unsafe { raw::syscall6(n, a(0), a(1), a(2), a(3), a(4), a(5)) as i64 }
}

fn clock_ns(clock: i32) -> i64 {
    let mut t = [0i64; 2];
    if sc(nr::CLOCK_GETTIME, &[clock as u64, t.as_mut_ptr() as u64]) != 0 { return 0; }
    t[0] * 1_000_000_000 + t[1]
}

fn say(line: &str) {
    let text = format!("\n{line}\n");
    sc(nr::WRITE, &[1, text.as_ptr() as u64, text.len() as u64]);
}

fn this_cpu() -> u32 {
    let mut cpu = u32::MAX;
    sc(nr::GETCPU, &[&mut cpu as *mut u32 as u64, 0, 0]);
    cpu
}

fn pin_to(cpu: u32) -> bool {
    let mask = 1u64 << cpu;
    if sc(nr::SCHED_SETAFFINITY, &[0, 8, &mask as *const u64 as u64]) != 0 { return false; }
    let start = clock_ns(CLOCK_MONOTONIC);
    while clock_ns(CLOCK_MONOTONIC) - start < 1_000_000_000 {
        if this_cpu() == cpu { return true; }
    }
    false
}

fn burns() {
    for i in 0..30 {
        let (m0, p0) = (clock_ns(CLOCK_MONOTONIC), clock_ns(CLOCK_PROCESS_CPUTIME_ID));
        let (mut last, mut worst) = (m0, 0);
        loop {
            let now = clock_ns(CLOCK_MONOTONIC);
            worst = worst.max(now - last);
            last = now;
            if now - m0 >= 300_000_000 { break; }
        }
        let p1 = clock_ns(CLOCK_PROCESS_CPUTIME_ID);
        say(&format!("STALL BURN i={i} wall_us={} cpu_us={} maxgap_us={}", (last - m0) / 1000, (p1 - p0) / 1000, worst / 1000));
    }
}

fn pinned(cpu: u32, ready: Arc<AtomicU32>, total: u32) -> String {
    if !pin_to(cpu) { ready.fetch_add(1, Ordering::SeqCst); return format!("STALL CPU cpu={cpu} pin=failed"); }
    ready.fetch_add(1, Ordering::SeqCst);
    while ready.load(Ordering::SeqCst) < total {}
    let start = clock_ns(CLOCK_MONOTONIC);
    let (mut m, mut c) = (start, clock_ns(CLOCK_THREAD_CPUTIME_ID));
    let (mut iterations, mut worst, mut gaps, mut charged_gaps, mut worst_uncharged) = (0u64, 0i64, 0u32, 0u32, 0i64);
    loop {
        let (m1, c1) = (clock_ns(CLOCK_MONOTONIC), clock_ns(CLOCK_THREAD_CPUTIME_ID));
        iterations += 1;
        let (gap, charged) = (m1 - m, c1 - c);
        if gap > worst { worst = gap; }
        if gap >= GAP_NS {
            gaps += 1;
            if charged * 2 >= gap { charged_gaps += 1; } else { worst_uncharged = worst_uncharged.max(gap); }
            say(&format!("STALL GAP cpu={cpu} t_ms={} gap_us={} charged_us={} rt_ms={}", (m - start) / 1_000_000, gap / 1000, charged / 1000, clock_ns(CLOCK_REALTIME) / 1_000_000));
            m = clock_ns(CLOCK_MONOTONIC);
            c = clock_ns(CLOCK_THREAD_CPUTIME_ID);
        } else {
            m = m1;
            c = c1;
        }
        if m - start >= PINNED_NS { break; }
    }
    format!("STALL CPU cpu={cpu} iterations={iterations} max_gap_us={} gaps={gaps} charged_gaps={charged_gaps} max_uncharged_us={}", worst / 1000, worst_uncharged / 1000)
}

fn measured() -> CaseResult { Ok(()) }

static SUITE: Suite = suite("stall", "Stall (scratch)", &[
    category("measure", "Measurement", &[
        case("done", "The stall measurement ran", measured),
    ]),
]);

fn measure() {
    say(&format!("STALL BEGIN rt_ms={}", clock_ns(CLOCK_REALTIME) / 1_000_000));
    burns();
    let total = 4;
    let ready = Arc::new(AtomicU32::new(0));
    let threads: Vec<_> = (0..total).map(|cpu| {
        let r = ready.clone();
        std::thread::spawn(move || pinned(cpu, r, total))
    }).collect();
    for t in threads {
        let line = t.join().unwrap_or_else(|_| "STALL CPU panicked".to_string());
        say(&line);
    }
    say(&format!("STALL END rt_ms={}", clock_ns(CLOCK_REALTIME) / 1_000_000));
}

fn main() {
    // PID 1 may not create threads, so a child process measures.
    match process::fork() {
        Ok(ForkResult::Child) => { measure(); process::exit(0) }
        Ok(ForkResult::Parent(pid)) => {
            let mut status = 0;
            let _ = process::waitpid(pid.raw() as i32, &mut status, 0);
        }
        Err(_) => say("STALL fork failed"),
    }
    SUITE.run()
}
