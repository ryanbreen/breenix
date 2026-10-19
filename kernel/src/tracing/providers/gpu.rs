//! The GPU core's global gauges (`docs/gui/gpu.md` section 1.16), read through
//! `/proc/trace/counters`, and the GPU-path marking the IRQ-masked gauge uses.
//!
//! `gpu_contexts`, `gpu_resources` and `gpu_resource_bytes` are gauges: a change is a
//! signed delta added to CPU 0's slot, so the slot, and the total `/proc/trace/counters`
//! prints, is the value itself, without a reset in between that a reader could catch.
//!
//! `gpu_irq_masked_max_us` is the longest interval a CPU held IRQs masked while it ran a GPU
//! path, kept in CPU 0's slot by `fetch_max`. A thread is in a GPU path while it holds a
//! [`GpuPath`]; the IRQ-masking primitives (`without_interrupts` and `IrqSafeMutex`) time
//! the intervals they open on such a thread, and the GPU interrupt handler times itself.

use core::sync::atomic::{AtomicU32, Ordering};

use alloc::sync::Arc;

use crate::signal::ThreadSignals;
use crate::tracing::counter::{register_counter, TraceCounter};

/// Contexts the GPU core holds, including destroyed handles whose work is outstanding.
#[no_mangle]
pub static GPU_CONTEXTS: TraceCounter = TraceCounter::new("gpu_contexts", "GPU contexts the core holds");

/// Resources the GPU core holds, its own included, and dropped ones still held.
#[no_mangle]
pub static GPU_RESOURCES: TraceCounter = TraceCounter::new("gpu_resources", "GPU resources the core holds");

/// Bytes charged to processes' GPU memory quotas and not yet released.
#[no_mangle]
pub static GPU_RESOURCE_BYTES: TraceCounter =
    TraceCounter::new("gpu_resource_bytes", "Bytes charged to GPU memory quotas");

/// The longest IRQ-masked interval in the GPU paths, microseconds, since boot or the last
/// `DISARM_ALL`.
#[no_mangle]
pub static GPU_IRQ_MASKED_MAX_US: TraceCounter =
    TraceCounter::new("gpu_irq_masked_max_us", "Longest IRQ-masked interval in the GPU paths, microseconds");

/// Threads holding a [`GpuPath`]: the masking primitives look no further while it is 0.
static GPU_PATH_THREADS: AtomicU32 = AtomicU32::new(0);

pub fn init() {
    for counter in [&GPU_CONTEXTS, &GPU_RESOURCES, &GPU_RESOURCE_BYTES, &GPU_IRQ_MASKED_MAX_US] {
        assert!(register_counter(counter).is_some(), "trace counter registry overflow while registering the GPU gauges");
    }
}

/// Change a gauge by `delta`.
#[inline]
pub fn gauge(counter: &TraceCounter, delta: i64) {
    counter.per_cpu[0].value.fetch_add(delta as u64, Ordering::Relaxed);
}

/// Restart the IRQ-masked gauge at 0 (`DISARM_ALL`).
pub fn reset_irq_masked() {
    GPU_IRQ_MASKED_MAX_US.per_cpu[0].value.store(0, Ordering::Relaxed);
}

#[inline(always)]
fn counter_now() -> u64 {
    let v: u64;
    // SAFETY: the virtual counter is readable at EL1.
    unsafe { core::arch::asm!("mrs {}, cntvct_el0", out(reg) v, options(nomem, nostack)) };
    v
}

/// Raise the IRQ-masked gauge to the interval since counter value `start`.
#[inline]
fn record_since(start: u64) {
    let ticks = counter_now().wrapping_sub(start);
    let freq = crate::arch_impl::aarch64::timer::frequency_hz().max(1);
    let us = ticks.saturating_mul(1_000_000) / freq;
    GPU_IRQ_MASKED_MAX_US.per_cpu[0].value.fetch_max(us, Ordering::Relaxed);
}

/// Called by a masking primitive just after it masks IRQs, with the DAIF it saved: the
/// counter value to time the interval from, or 0 when the interval is not timed (IRQs were
/// already masked, so this call opens no interval, or the current thread is in no GPU path).
#[inline(always)]
pub fn masked_begin(saved_daif: u64) -> u64 {
    const DAIF_I: u64 = 1 << 7;
    if saved_daif & DAIF_I != 0 || GPU_PATH_THREADS.load(Ordering::Relaxed) == 0 {
        return 0;
    }
    masked_begin_thread()
}

#[inline(never)]
fn masked_begin_thread() -> u64 {
    if !crate::per_cpu::is_initialized() || crate::per_cpu::in_interrupt() {
        return 0;
    }
    match crate::per_cpu::current_thread() {
        Some(thread) if thread.signals.gpu_path.load(Ordering::Relaxed) > 0 => counter_now().max(1),
        _ => 0,
    }
}

/// Called by the masking primitive before it restores IRQs, with `masked_begin`'s value.
#[inline(always)]
pub fn masked_end(start: u64) {
    if start != 0 {
        record_since(start);
    }
}

/// Time the GPU interrupt handler: its entry's counter value, for [`interrupt_end`].
#[inline(always)]
pub fn interrupt_begin() -> u64 {
    counter_now()
}

#[inline(always)]
pub fn interrupt_end(start: u64) {
    record_since(start);
}

/// The current thread is in a GPU path while this is held: syscall 416, `kgpu`, and the
/// exit and exec reclamation. The mark lives with the thread, so a thread that blocks or
/// moves to another CPU keeps it, and a thread that runs meanwhile does not inherit it.
pub struct GpuPath(Option<Arc<ThreadSignals>>);

impl GpuPath {
    pub fn enter() -> GpuPath {
        let signals = crate::arch_without_interrupts(|| crate::per_cpu::current_thread().map(|t| t.signals.clone()));
        if let Some(s) = &signals {
            if s.gpu_path.fetch_add(1, Ordering::Relaxed) == 0 {
                GPU_PATH_THREADS.fetch_add(1, Ordering::Relaxed);
            }
        }
        GpuPath(signals)
    }
}

impl Drop for GpuPath {
    fn drop(&mut self) {
        if let Some(s) = self.0.take() {
            if s.gpu_path.fetch_sub(1, Ordering::Relaxed) == 1 {
                GPU_PATH_THREADS.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
}
