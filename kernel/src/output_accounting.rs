//! Kernel output accounting (`docs/gui/breengel.md` section 22, `docs/gui/plan.md` section 12.1).
//!
//! A byte the kernel commits for output crosses one accounting boundary, which adds it to
//! the producing thread's `kernel_output_bytes` (read as `/proc/<pid>/task/<tid>/kernel_output`),
//! or to the trace counter `kernel_output_unattributed` where the producing thread cannot be
//! read safely: before per-CPU data exists, in interrupt context, on the panic path, and from
//! `emergency_print`. On aarch64 the boundaries are `serial_line`'s record emit and `try_write`,
//! and the posting of a `log_notice` line, which counts toward the poster. Writing posted lines
//! out is a drain and is not counted, and neither are the lead newlines `serial_writer` inserts
//! before a record. Userspace output (`Writer::Tty`, `Writer::Console`) is not kernel output.
//!
//! The boundary is lock-free: one relaxed atomic add, with interrupts masked only to read the
//! current thread.

use core::sync::atomic::{AtomicBool, Ordering};

/// Set once a panic has begun: from then on output is unattributed.
static PANICKING: AtomicBool = AtomicBool::new(false);

/// Count `len` bytes committed for output now. `attributed` is false for a writer that names
/// no thread (`emergency_print`).
#[inline]
pub fn count(len: usize, attributed: bool) {
    if len == 0 {
        return;
    }
    let len = len as u64;
    if attributed && !PANICKING.load(Ordering::Relaxed) && crate::per_cpu::is_initialized() {
        let counted = crate::arch_without_interrupts(|| {
            if crate::per_cpu::in_interrupt() {
                return false;
            }
            match crate::per_cpu::current_thread() {
                Some(thread) => {
                    thread.signals.kernel_output_bytes.fetch_add(len, Ordering::Relaxed);
                    true
                }
                None => false,
            }
        });
        if counted {
            return;
        }
    }
    crate::tracing::providers::output::count_unattributed(len);
}

/// A panic has begun on some CPU.
pub fn panicking() {
    PANICKING.store(true, Ordering::Relaxed);
}

/// Count posted `log_notice` lines toward the thread that posts them.
pub fn init() {
    crate::log_notice::set_on_post(|len| count(len, true));
}

/// Read the panic-state flag used by serial record admission.
pub fn is_panicking() -> bool { PANICKING.load(Ordering::Relaxed) }
