use core::sync::atomic::{AtomicU64, Ordering};

use super::quiesce::Mode;
use super::rng::DrawVector;
use super::sites;
use super::stimulus;

static VIOLATIONS: AtomicU64 = AtomicU64::new(0);

fn profile() -> &'static str {
    // platform_config distinguishes hypervisors, but not QEMU's selected CPU model.
    "unknown"
}

pub fn emit_seed_line(seed: u64, mode: Mode, smp: usize) {
    emit_run(seed, 0, mode, smp);
}

pub fn emit_run(seed: u64, iterations: u64, mode: Mode, smp: usize) {
    crate::serial_println!(
        "[COREPROOF:RUN:v1:comp=A:seed=0x{:016x}:iters={}:sites_declared={}:sites_visited={}:mode={}:profile={}:smp={}:downgraded={}:violations={}]",
        seed,
        iterations,
        sites::DECLARED,
        sites::visited_count(),
        mode.name(),
        profile(),
        smp,
        stimulus::downgraded_count(),
        violation_count(),
    );
}

pub fn violation(seed: u64, iteration: u64, vector: &DrawVector, predicate: &str, detail: u64) {
    VIOLATIONS.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!(
        "[COREPROOF:VIOLATION:v1:comp=A:seed=0x{:016x}:iter={}:site={}:action={}:ticks={}:order={}:acpu={}:pred={}:detail={}]",
        seed,
        iteration,
        vector.site.name(),
        stimulus::effective_action(vector).name(),
        vector.ticks,
        vector.order.name(),
        vector.antagonist_cpu,
        predicate,
        detail,
    );
}

pub fn violation_count() -> u64 {
    VIOLATIONS.load(Ordering::Relaxed)
}
