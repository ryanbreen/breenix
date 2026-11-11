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
//! On x86, syscall 416 also times its inherited mask from dispatch through the first
//! interrupt enable, or dispatch return with the inherited mask intact. Primitive scopes continue
//! to sample later masked work independently.

use core::sync::atomic::{AtomicU32, Ordering};
#[cfg(target_arch = "x86_64")]
use core::sync::atomic::AtomicU64;

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

/// Native resource VRAM, including its guards and VM tables. Guest backing
/// continues to use GPU_RESOURCE_BYTES and the existing per-process quota.
#[no_mangle]
pub static GPU_VRAM_BYTES: TraceCounter =
    TraceCounter::new("gpu_vram_bytes", "Native resource VRAM bytes retained");

/// The longest IRQ-masked interval in the GPU paths, microseconds, since boot or the last
/// `DISARM_ALL`.
#[no_mangle]
pub static GPU_IRQ_MASKED_MAX_US: TraceCounter =
    TraceCounter::new("gpu_irq_masked_max_us", "Longest IRQ-masked interval in the GPU paths, microseconds");

/// Valid PRESENT calls retained offscreen while their terminal is in the background.
#[no_mangle]
pub static GPU_PRESENTS_SUSPENDED: TraceCounter =
    TraceCounter::new("gpu_presents_suspended", "PRESENT calls retained without a device bind or flush");

/// Cumulative readback phase time, retained for failure-only procfs diagnostics.
pub static GPU_READ_POISON_NS: TraceCounter = TraceCounter::new("gpu_read_poison_ns", "Readback poison and cache clean nanoseconds");
pub static GPU_READ_WAIT_NS: TraceCounter = TraceCounter::new("gpu_read_wait_ns", "Readback device completion wait nanoseconds");
pub static GPU_READ_COMPARE_NS: TraceCounter = TraceCounter::new("gpu_read_compare_ns", "Readback invalidate copy and comparison nanoseconds");
pub static GPU_READ_PREPARE_NS: TraceCounter = TraceCounter::new("gpu_read_prepare_ns", "Readback validation and staging preparation nanoseconds");
pub static GPU_READ_OUTPUT_NS: TraceCounter = TraceCounter::new("gpu_read_output_ns", "Readback user output copy nanoseconds");

/// Long device waits distinguish notification delivery from watchdog polling.
pub static GPU_WAIT_LONG: TraceCounter = TraceCounter::new("gpu_wait_long", "GPU waits lasting at least 100 milliseconds");
pub static GPU_WAIT_SILENT: TraceCounter = TraceCounter::new("gpu_wait_silent", "Long GPU waits without an event count change");
pub static GPU_WAIT_SILENT_FOUND: TraceCounter = TraceCounter::new("gpu_wait_silent_found", "Completion found by first poll after a long silent wait");

pub static GPU_READ_PROBE_NS: TraceCounter = TraceCounter::new("gpu_read_probe_ns", "Readback user-page probing nanoseconds");
pub static GPU_READ_ALLOC_NS: TraceCounter = TraceCounter::new("gpu_read_alloc_ns", "Readback staging allocation nanoseconds");
pub static GPU_READ_WRITERS_NS: TraceCounter = TraceCounter::new("gpu_read_writers_ns", "Readback writer exclusion wait nanoseconds");

/// Threads holding a [`GpuPath`]: the masking primitives look no further while it is 0.
static GPU_PATH_THREADS: AtomicU32 = AtomicU32::new(0);

#[cfg(target_arch = "aarch64")]
mod gpu_mask {
    use alloc::{format, string::String};
    use core::panic::Location;
    use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

    struct Slot {
        open: AtomicPtr<Location<'static>>,
        sequence: AtomicU64,
        site: AtomicPtr<Location<'static>>,
        start: AtomicU64,
        us: AtomicU64,
        printed: AtomicU64,
    }

    static SLOTS: [Slot; crate::task::scheduler::MAX_CPUS] = [const { Slot {
        open: AtomicPtr::new(core::ptr::null_mut()),
        sequence: AtomicU64::new(0),
        site: AtomicPtr::new(core::ptr::null_mut()),
        start: AtomicU64::new(0),
        us: AtomicU64::new(0),
        printed: AtomicU64::new(0),
    } }; crate::task::scheduler::MAX_CPUS];

    /// Called on the owning CPU after masking; nested, untimed scopes do not call it.
    pub fn begin(site: &'static Location<'static>) {
        SLOTS[crate::task::scheduler::current_cpu()].open.store(core::ptr::from_ref(site).cast_mut(), Ordering::Relaxed);
    }

    /// Keep the source when the existing gauge raises its maximum. The writer has IRQs masked.
    pub fn raised(start: u64, us: u64) {
        let slot = &SLOTS[crate::task::scheduler::current_cpu()];
        slot.sequence.fetch_add(1, Ordering::AcqRel);
        slot.site.store(slot.open.load(Ordering::Relaxed), Ordering::Relaxed);
        slot.start.store(start, Ordering::Relaxed);
        slot.us.store(us, Ordering::Relaxed);
        slot.sequence.fetch_add(1, Ordering::Release);
    }

    /// Cold procfs diagnostic. Report only a coherent sample equal to the gauge read,
    /// once per start timestamp. A reset reading of 0 suppresses retained samples.
    pub fn report(maximum: u64) -> String {
        let mut output = String::new();
        if maximum <= 500 { return output; }
        for (cpu, slot) in SLOTS.iter().enumerate() {
            let sequence = slot.sequence.load(Ordering::Acquire);
            if sequence & 1 != 0 { continue; }
            let site = slot.site.load(Ordering::Relaxed);
            let start = slot.start.load(Ordering::Relaxed);
            let us = slot.us.load(Ordering::Relaxed);
            core::sync::atomic::fence(Ordering::Acquire);
            if slot.sequence.load(Ordering::Relaxed) != sequence || us != maximum || site.is_null() {
                continue;
            }
            if slot.printed.swap(start, Ordering::Relaxed) == start { continue; }
            // SAFETY: begin accepts only a static Location; the pointed-to object has static lifetime.
            let site = unsafe { &*site };
            output.push_str(&format!("GPU_MASKED cpu={cpu} start={start} us={us} at={}:{}\n", site.file(), site.line()));
        }
        output
    }
}

/// Read source attribution on the cold procfs path, after the gauge is sampled.
#[cfg(target_arch = "aarch64")]
pub fn irq_masked_sites(maximum: u64) -> alloc::string::String {
    gpu_mask::report(maximum)
}

/// Cold scalar diagnostics do not consume the fixed trace registry's slots.
pub fn readback_diagnostics() -> alloc::string::String {
    use core::fmt::Write;
    let mut out = alloc::string::String::new();
    for counter in [&GPU_READ_POISON_NS, &GPU_READ_WAIT_NS, &GPU_READ_COMPARE_NS,
        &GPU_READ_PREPARE_NS, &GPU_READ_OUTPUT_NS, &GPU_WAIT_LONG, &GPU_WAIT_SILENT,
        &GPU_WAIT_SILENT_FOUND, &GPU_READ_PROBE_NS, &GPU_READ_ALLOC_NS, &GPU_READ_WRITERS_NS] {
        let _ = writeln!(out, "{}: {}", counter.name, counter.aggregate());
    }
    out
}

pub fn init() {
    for counter in [&GPU_CONTEXTS, &GPU_RESOURCES, &GPU_RESOURCE_BYTES, &GPU_VRAM_BYTES, &GPU_IRQ_MASKED_MAX_US, &GPU_PRESENTS_SUSPENDED] {
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
    crate::arch_read_timestamp()
}

/// Raise the IRQ-masked gauge to the interval since counter value `start`.
#[inline]
fn record_since(start: u64) {
    let ticks = counter_now().wrapping_sub(start);
    let us = crate::arch_ticks_to_nanos(ticks) / 1_000;
    let previous = GPU_IRQ_MASKED_MAX_US.per_cpu[0].value.fetch_max(us, Ordering::Relaxed);
    #[cfg(target_arch = "aarch64")]
    if us > previous { gpu_mask::raised(start, us); }
    #[cfg(target_arch = "x86_64")]
    let _ = previous;
}

// Only the owning CPU accesses an open interval: syscall entry has IF clear and
// preemption held. Clear the slot before enabling IRQs, when blocking/migration
// becomes possible. No thread lookup, lock or allocation is needed at either end.
#[cfg(target_arch = "x86_64")]
static SYSCALL_MASK_START: [AtomicU64; crate::task::scheduler::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::task::scheduler::MAX_CPUS];

/// Cover syscall 416's inherited mask, including work before CORE is acquired.
#[cfg(target_arch = "x86_64")]
pub struct X86SyscallMask {
    cpu: usize,
    start: u64,
}

#[cfg(target_arch = "x86_64")]
impl X86SyscallMask {
    #[inline]
    pub fn enter() -> Self {
        let cpu = crate::per_cpu::cpu_id();
        let start = if crate::arch_interrupts_enabled() { 0 } else { counter_now().max(1) };
        SYSCALL_MASK_START[cpu].store(start, Ordering::Relaxed);
        Self { cpu, start }
    }
}

#[cfg(target_arch = "x86_64")]
impl Drop for X86SyscallMask {
    #[inline]
    fn drop(&mut self) {
        // After an enable this syscall may have slept and migrated. Close only
        // this syscall's interval on the original CPU at dispatch return.
        if self.start != 0 && SYSCALL_MASK_START[self.cpu]
            .compare_exchange(self.start, 0, Ordering::Relaxed, Ordering::Relaxed).is_ok()
        {
            record_since(self.start);
        }
    }
}

/// Close the inherited span immediately before the GPU seam first sets IF.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn syscall_mask_first_enable() {
    let start = SYSCALL_MASK_START[crate::per_cpu::cpu_id()].swap(0, Ordering::Relaxed);
    if start != 0 {
        record_since(start);
    }
}

/// Called just after a primitive masks IRQs, with its saved mask (bit 7 set if masked).
/// Return a timestamp for a GPU-path thread, or 0 outside one. ARM times only intervals
/// the primitive opens. x86 also times scopes under an inherited mask: syscall entry
/// clears IF before GpuPath is entered, so excluding those scopes hides IRQ_HOLD and
/// native critical sections. Overlapping samples contribute to the maximum via fetch_max.
#[inline(always)]
#[track_caller]
pub fn masked_begin(saved_daif: u64) -> u64 {
    const DAIF_I: u64 = 1 << 7;
    if (saved_daif & DAIF_I != 0 && !cfg!(target_arch = "x86_64"))
        || GPU_PATH_THREADS.load(Ordering::Relaxed) == 0
    {
        return 0;
    }
    masked_begin_thread()
}

#[inline(never)]
#[track_caller]
fn masked_begin_thread() -> u64 {
    if !crate::per_cpu::is_initialized() || crate::per_cpu::in_interrupt() {
        return 0;
    }
    match crate::per_cpu::current_thread() {
        Some(thread) if thread.signals.gpu_path.load(Ordering::Relaxed) > 0 => {
            #[cfg(target_arch = "aarch64")]
            gpu_mask::begin(core::panic::Location::caller());
            counter_now().max(1)
        }
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
#[track_caller]
pub fn interrupt_begin() -> u64 {
    #[cfg(target_arch = "aarch64")]
    gpu_mask::begin(core::panic::Location::caller());
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
