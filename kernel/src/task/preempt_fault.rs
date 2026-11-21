//! Failure-only preemption receipts (E24 ledger 703).
//!
//! An unpaired `preempt_enable()`, or a kernel thread that reaches
//! `kthread_exit` with a level still held, posts one line to the log notice
//! list, which the thread-context drain writes out. Posting masks interrupts
//! to keep the current thread's identity stable through publication. It
//! takes no lock and does not allocate, so it can run on the preemption paths
//! themselves. A balanced path does not reach it. claim-lint:ok:
//! kernel/src/log_notice.rs (Record::post_again formats into static storage
//! and publishes with a compare-exchange list push).

#[cfg(target_arch = "aarch64")]
use crate::arch_impl::current::percpu::Aarch64PerCpu as Cpu;
#[cfg(target_arch = "x86_64")]
use crate::arch_impl::current::percpu::X86PerCpu as Cpu;
use crate::arch_impl::PerCpuOps;
use crate::log_notice::Record;

static UNDERFLOW: Record = Record::new();

/// `preempt_enable()` found no PREEMPT level to release. The caller has
/// already added this release to the underflow total.
#[cold]
pub(crate) fn underflow(count: u32) {
    // The underflow total already counts this release if the detail is busy.
    post_record(&UNDERFLOW, "underflow", count, crate::per_cpu::preempt_underflow_count());
}

/// Post a kthread exit fault or count it for the notice drain if its detail
/// record is busy.
#[cold]
pub(super) fn post(record: &'static Record, reason: &str, count: u32, total: u64) {
    if !post_record(record, reason, count, total) {
        crate::log_notice::record_suppressed_kthread_exit();
    }
}

#[cold]
fn post_record(record: &'static Record, reason: &str, count: u32, total: u64) -> bool {
    crate::arch_without_interrupts(|| {
        let thread = Cpu::current_thread_ptr().cast::<super::thread::Thread>();
        let (tid, name) = if thread.is_null() {
            (0, "-")
        } else {
            // The installed thread cannot be switched out with interrupts
            // masked. Read only its identity fields, without borrowing Thread.
            unsafe { ((*thread).id, (&*(&raw const (*thread).name)).as_str()) }
        };
        let mut name_bytes = [0; 32];
        let len = name.len().min(name_bytes.len());
        for (byte, out) in name.bytes().zip(&mut name_bytes) {
            *out = if byte.is_ascii_graphic() { byte } else { b'_' };
        }
        let name = core::str::from_utf8(&name_bytes[..len]).unwrap_or("-");
        record.post_again(format_args!(
            "[PREEMPT_FAULT reason={} tid={} name={} count={:#x} underflows={}]",
            reason, tid, name, count, total,
        ))
    })
}
