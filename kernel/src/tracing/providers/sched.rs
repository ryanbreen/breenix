//! Scheduler trace provider.
//!
//! This provider traces scheduler and context switch events.
//! Events capture thread IDs and scheduling decisions.
//!
//! # Event Types
//!
//! - `CTX_SWITCH_ENTRY` (0x0001): Context switch beginning, payload = packed(old_tid, new_tid)
//! - `CTX_SWITCH_EXIT` (0x0002): Context switch complete, payload = new_tid
//! - `SCHED_PICK` (0x0200): Scheduler picked a thread, payload = thread_id
//! - `SCHED_RESCHED` (0x0201): Reschedule requested, payload = 0
//! - `SCHED_QUEUE_STATE` (0x0012): Queue state snapshot, payload = packed(ready_queue_len, chosen_tid)
//!
//! # Usage
//!
//! ```rust,ignore
//! use kernel::tracing::providers::sched::{SCHED_PROVIDER, CTX_SWITCH_ENTRY};
//! use kernel::trace_event_2;
//!
//! // Enable context switch tracing
//! SCHED_PROVIDER.enable_probe(0); // CTX_SWITCH_ENTRY
//!
//! // In context switch code:
//! trace_event_2!(SCHED_PROVIDER, CTX_SWITCH_ENTRY, old_tid as u16, new_tid as u16);
//! ```

use crate::tracing::counter::TraceCounter;
use crate::tracing::provider::{register_provider, TraceProvider};
use crate::tracing::providers::counters::{
    CTX_SWITCH_TOTAL, DISPATCH_EXC_IDLE_REDIRECT,
    DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_MANDATORY,
    DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_PREEMPT,
    DISPATCH_NOPROGRESS_SAVE_KTHREAD_MANDATORY, DISPATCH_NOPROGRESS_SAVE_KTHREAD_PREEMPT,
    DISPATCH_NOPROGRESS_SAVE_USER_MANDATORY, DISPATCH_NOPROGRESS_SAVE_USER_PREEMPT,
    DISPATCH_SAVE_REASON_KERNEL_BLOCKED_MANDATORY, DISPATCH_SAVE_REASON_KERNEL_BLOCKED_PREEMPT,
    DISPATCH_SAVE_REASON_KTHREAD_MANDATORY, DISPATCH_SAVE_REASON_KTHREAD_PREEMPT,
    DISPATCH_SAVE_REASON_USER_MANDATORY, DISPATCH_SAVE_REASON_USER_PREEMPT,
    DISPATCH_SWITCH_IDLE_REDIRECT, DISPATCH_SWITCH_ROLLED_BACK,
};
use core::sync::atomic::AtomicU64;

/// Provider ID for scheduler events.
/// Uses 0x00 for context switch events (0x00xx) and 0x02 for scheduler events (0x02xx).
/// For simplicity, we use a single provider with both ranges.
pub const PROVIDER_ID: u8 = 0x00;

/// Scheduler trace provider.
///
/// GDB: `print SCHED_PROVIDER`
#[no_mangle]
pub static SCHED_PROVIDER: TraceProvider = TraceProvider {
    name: "sched",
    id: PROVIDER_ID,
    enabled: AtomicU64::new(0),
};

// =============================================================================
// Context Switch Probes (0x00xx range)
// =============================================================================

/// Probe ID for context switch entry.
pub const PROBE_CTX_SWITCH_ENTRY: u8 = 0x01;

/// Probe ID for context switch exit.
pub const PROBE_CTX_SWITCH_EXIT: u8 = 0x02;

/// Probe ID for switch to userspace.
pub const PROBE_CTX_SWITCH_TO_USER: u8 = 0x03;

/// Probe ID for switch to kernel.
pub const PROBE_CTX_SWITCH_TO_KERNEL: u8 = 0x04;

/// Probe ID for switch to idle.
pub const PROBE_CTX_SWITCH_TO_IDLE: u8 = 0x05;

/// Event type for context switch entry.
/// Payload: packed(old_tid[15:0], new_tid[15:0]).
pub const CTX_SWITCH_ENTRY: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_CTX_SWITCH_ENTRY as u16);

/// Event type for context switch exit.
/// Payload: new_tid.
pub const CTX_SWITCH_EXIT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_CTX_SWITCH_EXIT as u16);

/// Event type for switch to userspace.
/// Payload: thread_id.
pub const CTX_SWITCH_TO_USER: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_CTX_SWITCH_TO_USER as u16);

/// Event type for switch to kernel.
/// Payload: thread_id.
pub const CTX_SWITCH_TO_KERNEL: u16 =
    ((PROVIDER_ID as u16) << 8) | (PROBE_CTX_SWITCH_TO_KERNEL as u16);

/// Event type for switch to idle.
/// Payload: 0.
pub const CTX_SWITCH_TO_IDLE: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_CTX_SWITCH_TO_IDLE as u16);

// =============================================================================
// Scheduler Probes (using upper probe IDs to avoid collision)
// =============================================================================

/// Probe ID for scheduler pick.
pub const PROBE_SCHED_PICK: u8 = 0x10;

/// Probe ID for reschedule request.
pub const PROBE_SCHED_RESCHED: u8 = 0x11;

/// Probe ID for scheduler queue state snapshot.
pub const PROBE_SCHED_QUEUE_STATE: u8 = 0x12;

/// Event type for scheduler picking a thread.
/// Payload: thread_id.
pub const SCHED_PICK: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_SCHED_PICK as u16);

/// Event type for reschedule request.
/// Payload: 0.
pub const SCHED_RESCHED: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_SCHED_RESCHED as u16);

/// Event type for scheduler queue state snapshot.
/// Payload: packed(ready_queue_len, chosen_tid).
pub const SCHED_QUEUE_STATE: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_SCHED_QUEUE_STATE as u16);

// =============================================================================
// Initialization
// =============================================================================

/// Initialize the scheduler provider.
///
/// Registers the provider with the global registry.
pub fn init() {
    register_provider(&SCHED_PROVIDER);
}

// =============================================================================
// Inline Tracing Functions
// =============================================================================

/// Trace context switch entry (inline for minimal overhead).
///
/// Also increments the CTX_SWITCH_TOTAL counter (single atomic add, always runs).
///
/// # Parameters
///
/// - `old_tid`: Thread ID of the thread being switched from
/// - `new_tid`: Thread ID of the thread being switched to
#[inline(always)]
#[allow(dead_code)]
pub fn trace_ctx_switch(old_tid: u64, new_tid: u64) {
    // Always increment the counter (single atomic add, ~3 cycles)
    CTX_SWITCH_TOTAL.increment();

    // Only record trace event if tracing is enabled
    if SCHED_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event_2(CTX_SWITCH_ENTRY, old_tid as u16, new_tid as u16);
    }
}

/// Trace switch to idle (inline for minimal overhead).
#[inline(always)]
#[allow(dead_code)]
pub fn trace_switch_to_idle() {
    if SCHED_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(CTX_SWITCH_TO_IDLE, 0, 0);
    }
}

/// Trace scheduler queue state (inline for minimal overhead).
///
/// Records ready queue length and the chosen thread ID when a
/// scheduling decision is made.
///
/// # Parameters
///
/// - `ready_queue_len`: Number of threads in the ready queue
/// - `chosen_tid`: Thread ID of the thread chosen to run
#[inline(always)]
#[allow(dead_code)]
pub fn trace_sched_queue_state(ready_queue_len: u16, chosen_tid: u16) {
    if SCHED_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event_2(SCHED_QUEUE_STATE, ready_queue_len, chosen_tid);
    }
}

// =============================================================================
// Dispatch save census (#772 diagnostics, R111/R112)
// =============================================================================
//
// Q1 of the #772 instrumentation lane: WHICH path produces each
// identical-RIP/RSP restore->save pair. The enum below is the single place the
// reason codes are defined; `kernel/src/interrupts/context_switch.rs` names one
// at each of its three save sites, and the helper turns that into two counter
// increments and one trace event. Everything here is lock-free, allocation-free
// and formatting-free, so it is legal on the interrupt-return path.

/// Probe ID for a context save performed on the dispatch path.
pub const PROBE_DISPATCH_SAVE: u8 = 0x20;

/// Probe ID for a dispatch abandoned before the dispatched thread ran.
pub const PROBE_DISPATCH_ABANDON: u8 = 0x21;

/// Event type for a context save on the dispatch path.
///
/// Flags: the `DispatchSaveReason` discriminant in bits 0-6, with bit 7 set
/// when the frame being saved is byte-identical to the frame the last
/// completed dispatch installed for the same thread.
/// Payload: the low 32 bits of the saved RIP.
pub const DISPATCH_SAVE: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_DISPATCH_SAVE as u16);

/// Event type for a dispatch abandoned inside the switch path.
///
/// Flags: the `DispatchAbandonSite` discriminant. Payload: 0.
pub const DISPATCH_ABANDON: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_DISPATCH_ABANDON as u16);

/// Which path produced a context save on the x86 dispatch path (#772).
///
/// One value per (save flavour x admitting gate) pair. The save flavour is
/// which of `check_need_resched_and_switch`'s three save arms ran; the
/// admitting gate is why that call was allowed to switch at all:
///
/// * `*Mandatory` -- `current_thread_blocked_or_terminated` was true, so the
///   switch is the one the scheduler must perform. The #772 refusal is
///   conjoined out of this arm, so a no-progress save counted here is one the
///   refusal's `if !current_thread_blocked_or_terminated` excludes.
/// * `*Preempt` -- the current thread was neither blocked nor terminated, so
///   gate 4 admitted on `need_resched`. This is the arm the refusal guards.
///
/// The two are read off the same boolean the refusal itself uses, so the split
/// is exactly the refusal's own visibility boundary and not a second predicate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum DispatchSaveReason {
    /// Userspace frame saved by `save_current_thread_context_with_guard`,
    /// admitted on `need_resched`.
    UserPreempt = 0,
    /// Userspace frame saved by `save_current_thread_context_with_guard`,
    /// admitted because the current thread is blocked or terminated.
    UserMandatory = 1,
    /// Blocked-in-syscall kernel frame saved by
    /// `save_kernel_context_with_guard` -- the save side of the #772 proxy --
    /// admitted on `need_resched`.
    KernelBlockedPreempt = 2,
    /// Blocked-in-syscall kernel frame saved by
    /// `save_kernel_context_with_guard`, admitted because the current thread is
    /// blocked or terminated.
    KernelBlockedMandatory = 3,
    /// Pure kernel thread frame saved by `save_kthread_context`, admitted on
    /// `need_resched`.
    KthreadPreempt = 4,
    /// Pure kernel thread frame saved by `save_kthread_context`, admitted
    /// because the current thread is blocked or terminated.
    KthreadMandatory = 5,
}

impl DispatchSaveReason {
    /// The (total, no-progress) counter pair this reason increments.
    #[inline(always)]
    fn counters(self) -> (&'static TraceCounter, &'static TraceCounter) {
        match self {
            Self::UserPreempt => (
                &DISPATCH_SAVE_REASON_USER_PREEMPT,
                &DISPATCH_NOPROGRESS_SAVE_USER_PREEMPT,
            ),
            Self::UserMandatory => (
                &DISPATCH_SAVE_REASON_USER_MANDATORY,
                &DISPATCH_NOPROGRESS_SAVE_USER_MANDATORY,
            ),
            Self::KernelBlockedPreempt => (
                &DISPATCH_SAVE_REASON_KERNEL_BLOCKED_PREEMPT,
                &DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_PREEMPT,
            ),
            Self::KernelBlockedMandatory => (
                &DISPATCH_SAVE_REASON_KERNEL_BLOCKED_MANDATORY,
                &DISPATCH_NOPROGRESS_SAVE_KERNEL_BLOCKED_MANDATORY,
            ),
            Self::KthreadPreempt => (
                &DISPATCH_SAVE_REASON_KTHREAD_PREEMPT,
                &DISPATCH_NOPROGRESS_SAVE_KTHREAD_PREEMPT,
            ),
            Self::KthreadMandatory => (
                &DISPATCH_SAVE_REASON_KTHREAD_MANDATORY,
                &DISPATCH_NOPROGRESS_SAVE_KTHREAD_MANDATORY,
            ),
        }
    }
}

/// Where a dispatch was abandoned after `schedule()` had already committed to
/// it (#772).
///
/// 0 of these 16 sites saves a context of its own: the outgoing thread was
/// already saved under one of the `DispatchSaveReason` values above, or, for
/// the 2 exception-handler sites, was not saved at all. They are recorded
/// because each one ends a dispatch, and the dispatch mark written when
/// `switch_to_thread` returns then describes whatever thread the CPU was left
/// running.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum DispatchAbandonSite {
    /// `check_need_resched_and_switch`: the userspace save failed.
    RollbackSaveFailed = 0,
    /// `switch_to_thread`: TLS switch failed.
    RollbackTls = 1,
    /// `restore_userspace_thread_context`: first userspace entry aborted.
    RollbackFirstEntry = 2,
    /// `switch_to_thread`: the blocked-in-syscall arm could not take the
    /// process-manager guard.
    RollbackKernelContextLock = 3,
    /// `switch_to_thread`, blocked-in-syscall arm: unpublished dispatch.
    IdleUnpublishedBlocked = 4,
    /// `restore_userspace_thread_context`: unpublished dispatch.
    IdleUnpublishedUser = 5,
    /// `switch_to_thread`, blocked-in-syscall arm: process has no CR3.
    IdleNoCr3Blocked = 6,
    /// `restore_userspace_thread_context`: process has no CR3.
    IdleNoCr3User = 7,
    /// `switch_to_thread`, blocked-in-syscall arm: a signal terminated the
    /// process during delivery.
    IdleSignalTerminatedBlocked = 8,
    /// `restore_userspace_thread_context`: a signal terminated the process
    /// during delivery.
    IdleSignalTerminatedUser = 9,
    /// `restore_userspace_thread_context`: the process was already terminated
    /// after a signal was delivered.
    IdleProcessTerminatedUser = 10,
    /// `restore_userspace_thread_context`: the saved userspace context was
    /// refused (non-canonical RIP/RSP, or a kernel frame).
    IdleRestoreError = 11,
    /// `kernel/src/interrupts.rs`: the page-fault handler redirected a
    /// terminated thread to idle.
    ExceptionPageFault = 12,
    /// `kernel/src/interrupts.rs`: the general-protection-fault handler
    /// redirected a terminated thread to idle.
    ExceptionGeneralProtection = 13,
    /// `check_and_deliver_signals_for_current_thread`: a signal terminated the
    /// process on an interrupt-return arm that performed no switch -- including
    /// the #772 refusal arm, which calls the same helper.
    IdleSignalTerminatedOnReturn = 14,
    /// `check_and_deliver_signals_for_current_thread`: the process was already
    /// terminated after a signal was delivered on a no-switch return arm.
    IdleProcessTerminatedOnReturn = 15,
}

impl DispatchAbandonSite {
    /// The counter this site increments. Sites are grouped three ways --
    /// rollback, idle redirect inside the switch path, and idle redirect from
    /// an exception handler -- with the exact site kept in the trace event's
    /// flags.
    #[inline(always)]
    fn counter(self) -> &'static TraceCounter {
        match self {
            Self::RollbackSaveFailed
            | Self::RollbackTls
            | Self::RollbackFirstEntry
            | Self::RollbackKernelContextLock => &DISPATCH_SWITCH_ROLLED_BACK,
            Self::ExceptionPageFault | Self::ExceptionGeneralProtection => {
                &DISPATCH_EXC_IDLE_REDIRECT
            }
            _ => &DISPATCH_SWITCH_IDLE_REDIRECT,
        }
    }
}

/// Record one context save on the dispatch path.
///
/// Two per-CPU atomic adds and, when the provider is enabled, one trace event.
/// No lock, no allocation, no formatting.
#[inline(always)]
pub fn trace_dispatch_save(reason: DispatchSaveReason, no_progress: bool, rip: u64) {
    let (total, no_progress_total) = reason.counters();
    total.increment();
    if no_progress {
        no_progress_total.increment();
    }
    if SCHED_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        let flags = (reason as u8) | if no_progress { 0x80 } else { 0x00 };
        crate::tracing::record_event(DISPATCH_SAVE, flags, rip as u32);
    }
}

/// Record one dispatch abandoned after `schedule()` committed to it.
#[inline(always)]
pub fn trace_dispatch_abandon(site: DispatchAbandonSite) {
    site.counter().increment();
    if SCHED_PROVIDER.is_enabled() && crate::tracing::is_enabled() {
        crate::tracing::record_event(DISPATCH_ABANDON, site as u8, 0);
    }
}
