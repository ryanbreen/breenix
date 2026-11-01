// The oracle, capture sections and byte writer are production code. Only the
// hardware clock/CPU, GPU readiness and scheduler acquisition are substituted.
extern crate self as spin;
pub struct Once<T>(std::sync::OnceLock<T>);
impl<T> Once<T> {
    pub const fn new() -> Self { Self(std::sync::OnceLock::new()) }
    pub fn call_once(&self, f: impl FnOnce() -> T) -> &T { self.0.get_or_init(f) }
    pub fn get(&self) -> Option<&T> { self.0.get() }
}
impl<T> Default for Once<T> {
    fn default() -> Self { Self::new() }
}

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
static CPU: AtomicUsize = AtomicUsize::new(3);
static BUSY: AtomicBool = AtomicBool::new(false);
static READS: AtomicUsize = AtomicUsize::new(0);
static GPU_READY: AtomicBool = AtomicBool::new(true);
static UPTIME: AtomicU64 = AtomicU64::new(11_040);
static WIRE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

#[cfg(target_arch = "aarch64")]
mod serial_line {
    pub struct Line;
    impl Line {
        pub fn new() -> Self { Self }
        pub fn char(&mut self, c: u8) { crate::WIRE.lock().unwrap().push(c as char); }
    }
}

mod time { pub mod timer {
    pub const MS_PER_TICK: u64 = 5;
    pub fn get_monotonic_time() -> u64 { crate::UPTIME.load(crate::Ordering::Relaxed) }
} }
mod arch_impl {
    pub trait PerCpuOps {
        fn cpu_id() -> u64;
        fn preempt_count() -> u32 { 65537 }
        fn need_resched() -> bool { true }
        fn in_hardirq() -> bool { true }
        fn in_softirq() -> bool { false }
    }
    pub mod current { pub mod percpu {
        #[cfg(target_arch = "x86_64")]
        pub struct X86PerCpu;
        #[cfg(target_arch = "x86_64")]
        impl crate::arch_impl::PerCpuOps for X86PerCpu {
            fn cpu_id() -> u64 { crate::CPU.load(crate::Ordering::Relaxed) as u64 }
        }
        #[cfg(target_arch = "aarch64")]
        pub struct Aarch64PerCpu;
        #[cfg(target_arch = "aarch64")]
        impl crate::arch_impl::PerCpuOps for Aarch64PerCpu {
            fn cpu_id() -> u64 { crate::CPU.load(crate::Ordering::Relaxed) as u64 }
        }
    } }
}
mod task { pub mod scheduler {
    #[derive(Clone, Copy, Default)]
    pub struct SchedulerLivenessSnapshot {
        pub per_cpu_current: [u64; 8],
        pub per_cpu_ready_len: [u64; 8],
        pub total_threads: u64,
        pub blocked_count: u64,
    }
    pub fn try_liveness_snapshot(_: usize) -> Option<SchedulerLivenessSnapshot> {
        crate::READS.fetch_add(1, crate::Ordering::Relaxed);
        if crate::BUSY.load(crate::Ordering::Relaxed) { return None; }
        Some(SchedulerLivenessSnapshot { per_cpu_current: [1260; 8],
            per_cpu_ready_len: [2; 8], total_threads: 27, blocked_count: 19 })
    }
} }
mod gpu {
    pub mod console { pub fn active() -> bool { true } }
    pub mod panic {
        pub fn test_ready() -> bool { crate::GPU_READY.load(crate::Ordering::Relaxed) }
        pub fn test_with_locks(f: impl FnOnce()) { f(); }
    }
}
mod tracing {
    use super::*;
    pub const MAX_CPUS: usize = 8;
    pub const TRACE_BUFFER_SIZE: usize = 2048;
    pub static TRACE_ENABLED: AtomicU64 = AtomicU64::new(1);
    pub struct Event { pub timestamp: u64, pub event_type: u16, pub payload: u32, pub flags: u16 }
    pub struct TraceCpuBuffer;
    impl TraceCpuBuffer {
        pub fn write_index(&self) -> usize { 0 }
        pub fn dropped_count(&self) -> u64 { 0 }
        pub fn iter_events(&self) -> std::iter::Empty<Event> { std::iter::empty() }
    }
    pub static mut TRACE_BUFFERS: [TraceCpuBuffer; 8] = [const { TraceCpuBuffer }; 8];
    pub fn event_type_name(_: u16) -> &'static str { "TIMER_TICK" }
    pub fn timestamp_to_nanos(t: u64) -> u64 { t }
    pub fn trace_timestamp() -> u64 { 235782522132 }
    pub fn timestamp_frequency_hz() -> u64 { 3600000000 }
    pub mod counter {
        pub struct Counter { pub name: &'static str }
        impl Counter { pub fn aggregate(&self) -> u64 { 2208 } }
        pub fn list_counters() -> &'static [Counter] { &[Counter { name: "TIMER_TICK_TOTAL" }] }
    }
    #[cfg(target_arch = "x86_64")]
    pub mod output {
        pub fn raw_serial_char(c: u8) { crate::WIRE.lock().unwrap().push(c as char); }
    }
}
#[path = "ORACLE_PATH"]
pub mod capture_oracle;
#[path = "CAPTURE_PATH"]
pub mod capture;

fn wire() -> String { std::mem::take(&mut *WIRE.lock().unwrap()) }
fn emit_panic() { capture::emit(capture::Edge::Panic, 95, 5); }
fn partial(text: &str) {
    assert!(text.contains("[BXCAP:NOTE sched_lock_held]"), "{text}");
    assert!(text.contains("verdict=partial"), "{text}");
    assert!(text.contains("sections_skipped=0x20"), "{text}");
    assert!(!text.contains("[BXCAP:THR"), "{text}");
}
fn complete(text: &str) {
    assert!(!text.contains("sched_lock_held"), "{text}");
    assert!(text.contains("verdict=complete"), "{text}");
    assert!(text.contains("truncated=0 sections_skipped=0x0"), "{text}");
    assert_eq!(text.matches("[BXCAP:THR").count(), 8, "{text}");
    assert!(text.contains("cur=1260 rq=2 threads=27 blocked=19 q=exact"), "{text}");
    assert!(!text.contains("thr_sample=panic_oracle"), "{text}");
}
fn invoke(tick: u64) -> bool {
    std::panic::catch_unwind(|| capture_oracle::observe(tick)).is_err()
}
fn hook(f: impl Fn() + Send + Sync + 'static) {
    std::panic::set_hook(Box::new(move |info| {
        if info.to_string().contains("capture_panic_oracle: deliberate panic") {
            f();
        } else {
            eprintln!("{info}");
        }
    }));
}
fn main() {
    // Keep the production enum's other edges compiled in this scoped harness.
    let _ = (capture::Edge::SelfTest, capture::Edge::Fault, capture::Edge::Lockup);
    let scenario = std::env::args().nth(1).unwrap();
    hook(|| {});
    if scenario == "real-panic" {
        BUSY.store(true, Ordering::Relaxed);
        emit_panic();
        partial(&wire());
        assert_eq!(READS.load(Ordering::Relaxed), 1);
    } else if scenario == "deadline" {
        GPU_READY.store(false, Ordering::Relaxed);
        BUSY.store(true, Ordering::Relaxed);
        hook(emit_panic);
        assert!(invoke(36_000), "the preparation deadline must still reach serial");
        partial(&wire());
        assert!(!invoke(36_001), "the deliberate panic is one-shot");
    } else if scenario == "busy" {
        BUSY.store(true, Ordering::Relaxed);
        assert!(!invoke(2208), "defer the oracle while its thread snapshot is unavailable");
        assert_eq!(READS.load(Ordering::Relaxed), 1, "one try, no IRQ spin loop");
        assert!(wire().is_empty());
        BUSY.store(false, Ordering::Relaxed);
        hook(emit_panic);
        assert!(invoke(2209), "deferral must not consume FIRED");
        complete(&wire());
    } else {
        let peer = scenario == "peer";
        assert!(peer || scenario == "banner-race");
        hook(move || {
            // A peer takes the lock after injection and before BXCAP's read,
            // reproducing the window in the retained failed panic capture.
            BUSY.store(true, Ordering::Relaxed);
            UPTIME.store(11_640, Ordering::Relaxed);
            if peer {
                CPU.store(2, Ordering::Relaxed);
                emit_panic();
                partial(&wire());
                CPU.store(3, Ordering::Relaxed);
            }
            emit_panic();
        });
        assert!(invoke(2208));
        partial(&wire());
        assert_eq!(READS.load(Ordering::Relaxed), if peer { 3 } else { 2 },
            "the real emitter must try its own nonblocking scheduler read");
        emit_panic();
        partial(&wire()); // Later captures also report live contention.
        assert!(!invoke(2209));
    }
    // Panic hooks also run for failed assertions; restore readable diagnostics.
    drop(std::panic::take_hook());
    println!("{scenario}: pass");
}
