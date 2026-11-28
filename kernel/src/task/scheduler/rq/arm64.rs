//! The ARM64 switch transaction, from selection through the stack pivot.

use super::*;
use crate::task::scheduler::{self, record_cpu_state_change, set_cpu_idle};
use crate::task::thread::ThreadPrivilege;

pub(crate) struct Dispatch(Held);

/// A queue guard entrusted to the stack-pivot trampoline.
#[repr(transparent)]
pub(crate) struct PivotToken(usize);

impl PivotToken {
    pub(crate) fn cpu(&self) -> usize {
        self.0
    }
}

impl Dispatch {
    /// The IRQ-return caller must leave its current context in place on a
    /// busy lock; a task-context handoff can retry with IRQs enabled.
    pub(crate) fn try_local(cpu: usize) -> Option<Self> {
        try_dispatch(cpu).map(Self)
    }

    pub(crate) fn cpu(&self) -> usize {
        self.0.cpu
    }

    pub(crate) fn has_ready_work() -> bool {
        let cpu = scheduler::current_cpu_id_raw();
        if scheduler::QUEUED_CPUS.load(Ordering::Acquire) & (1 << cpu) == 0 {
            return false;
        }
        let Some(held) = try_remote(cpu) else {
            return true;
        };
        let queue = unsafe { &*core::ptr::addr_of!((*held.sched).per_cpu_queues[cpu]) };
        queue.iter().copied().any(|tid| {
            !crate::arch_impl::aarch64::switch_save::is_saving(tid)
                && stack_home(&held, tid).is_none_or(|home| home == cpu)
        })
    }

    pub(crate) fn available() -> bool {
        live()
    }

    pub(crate) fn state(&mut self) -> &mut CpuSchedulerState {
        self.0.state()
    }

    pub(crate) fn state_ref(&self) -> &CpuSchedulerState {
        self.0.state_ref()
    }

    pub(crate) fn get_thread(&self, tid: u64) -> Option<&Thread> {
        self.0.thread_ref(tid)
    }

    pub(crate) fn get_thread_mut(&mut self, tid: u64) -> Option<&mut Thread> {
        self.0.thread(tid)
    }

    pub(crate) fn current_thread_mut(&mut self) -> Option<&mut Thread> {
        let tid = self.state().current_thread?;
        self.get_thread_mut(tid)
    }

    pub(crate) fn ready_queue_length(&mut self) -> usize {
        self.0.queue().len()
    }

    pub(crate) fn idle_thread(&self) -> u64 {
        self.state_ref().idle_thread
    }

    pub(crate) fn is_idle_thread_inner(&self, tid: u64) -> bool {
        self.idle_thread() == tid
    }

    pub(crate) fn remove_from_ready_queue(&mut self, tid: u64) {
        self.0.remove_from_ready_queue(tid);
    }

    // The token is carried as an assembly argument on the pivot, independent
    // of the scheduler-stack slot that a retraction can invalidate.
    pub(crate) fn into_pivot(self) -> PivotToken {
        let cpu = self.0.cpu;
        core::mem::forget(self);
        PivotToken(cpu)
    }

    /// # Safety
    /// The token from the forgotten guard, consumed once on the
    /// scheduler stack. IRQs have remained masked since it was acquired.
    pub(crate) unsafe fn from_pivot(token: PivotToken) -> Self {
        Self(Held {
            cpu: token.0,
            sched: LIVE.load(Ordering::Acquire),
        })
    }

    pub(crate) fn set_need_resched_inner(&self) {
        scheduler::set_need_resched();
    }

    pub(crate) fn resolve_pending_next_locked(&mut self) {
        let Some(tid) = self.state().pending_next else {
            return;
        };
        #[cfg(feature = "coreproof_mut_pending_next")]
        {
            let _ = tid;
            self.state().pending_next = None;
        }
        #[cfg(not(feature = "coreproof_mut_pending_next"))]
        {
            if self.is_idle_thread_inner(tid)
                || self.state().current_thread == Some(tid)
                || self.state().previous_thread == Some(tid)
                || self.0.queue().contains(&tid)
                || crate::arch_impl::aarch64::context_switch::deferred_requeue_contains(tid)
            {
                return;
            }
            if let Some(thread) = self.get_thread_mut(tid) {
                if matches!(thread.state, ThreadState::Running | ThreadState::Ready) {
                    thread.set_ready();
                    self.state().pending_next = None;
                    self.requeue_thread_after_save(tid);
                    scheduler::set_need_resched();
                    #[cfg(feature = "boot_tests")]
                    crate::task::strand_oracle::note_pending_next_resolved(tid);
                }
            }
        }
    }

    pub(crate) fn commit_cpu_state_after_save(&mut self, tid: u64) {
        let cpu = self.0.cpu;
        let old = self.state().current_thread;
        if old != Some(tid) {
            // Parking completion attributes the save to the outgoing TCB
            // while the hardware per-CPU pointer still identifies it.
            if let Some(thread) = unsafe { crate::task::thread::arm_current_thread_ptr().as_ref() }
            {
                if old == Some(thread.id()) && thread.state == ThreadState::BlockedOnIO {
                    thread.io_park_epoch.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        record_cpu_state_change(cpu, 1, old.unwrap_or(0xDEAD), tid);
        let state = self.state();
        state.current_thread = Some(tid);
        if state.pending_next == Some(tid) {
            state.pending_next = None;
        }
    }

    pub(crate) fn fix_stale_idle_cpu_state(&mut self, tid: u64) {
        if self.state_ref().current_thread == Some(self.idle_thread())
            && self.get_thread(tid).is_some()
        {
            self.remove_from_ready_queue(tid);
            self.commit_cpu_state_after_save(tid);
        }
    }

    pub(crate) fn fix_stale_current_thread_when_idle_executing(&mut self) {
        #[cfg(feature = "boot_tests")]
        scheduler::IDLE_INLINE_SELECT[self.0.cpu].store(true, Ordering::Relaxed);
        self.restore_idle(7);
    }

    pub(crate) fn fix_exception_cleanup_cpu_state(&mut self) {
        self.restore_idle(9);
        if let Some(tid) = self.state().previous_thread.take() {
            self.requeue_thread_after_save(tid);
        }
        set_cpu_idle(self.0.cpu, true);
    }

    fn restore_idle(&mut self, reason: u64) {
        let cpu = self.0.cpu;
        let idle = self.idle_thread();
        let state = self.state();
        let current = state.current_thread.unwrap_or(0xDEAD);
        if current != idle {
            record_cpu_state_change(cpu, reason, current, idle);
            state.current_thread = Some(idle);
        }
    }

    pub(crate) fn requeues_without_ipi(&self, tid: u64) -> bool {
        !self
            .get_thread(tid)
            .and_then(|t| t.cpu_affinity)
            .is_some_and(|pin| pin.per_cpu_worker && pin.cpu != self.0.cpu)
    }

    pub(crate) fn requeue_switched_out(&mut self, tid: u64) {
        if let Some(front) = self.prepare_requeue(tid) {
            // The exception-return tail owns only this queue and performs no
            // device access. The save mark survives until after SP moves.
            self.0.enqueue(tid, front);
        }
    }

    pub(crate) fn requeue_thread_after_save(&mut self, tid: u64) {
        self.requeue_switched_out(tid);
    }

    pub(crate) fn requeue_thread_on_cpu(&mut self, tid: u64, cpu: usize) {
        let Some(front) = self.prepare_requeue(tid) else {
            return;
        };
        if cpu == self.0.cpu {
            self.0.enqueue(tid, front);
        } else if let Some(mut remote) = try_remote(cpu) {
            let idle = remote.state_ref().current_thread == Some(remote.state_ref().idle_thread);
            remote.enqueue(tid, front);
            if idle {
                kick(cpu);
            }
        } else {
            // Keep the queued copy under this owner's lock until its stack's
            // home is available. Selection rejects a foreign stack slot.
            self.0.enqueue(tid, front);
        }
    }

    fn prepare_requeue(&mut self, tid: u64) -> Option<bool> {
        if self.state().previous_thread == Some(tid) {
            self.state().previous_thread = None;
        }
        if self.is_idle_thread_inner(tid)
            || self.state().current_thread == Some(tid)
            || self.state().pending_next == Some(tid)
            || self.0.queue().contains(&tid)
            || crate::arch_impl::aarch64::context_switch::deferred_requeue_contains(tid)
        {
            return None;
        }
        let thread = self.get_thread_mut(tid)?;
        if !matches!(thread.state, ThreadState::Ready | ThreadState::Running) {
            return None;
        }
        thread.set_ready();
        let front = core::mem::take(&mut thread.sched.head_on_requeue);
        scheduler::ENQUEUE_DEFERRED_DRAINED_OK.fetch_add(1, Ordering::Relaxed);
        Some(front)
    }

    pub(crate) fn schedule_deferred_requeue(&mut self) -> Option<(u64, u64, bool)> {
        let cpu = self.0.cpu;
        let idle = self.idle_thread();
        if let Some(thread) = unsafe { crate::task::thread::arm_current_thread_ptr().as_ref() } {
            thread.lockup_progress.record_schedule_entry();
        }
        self.state().last_schedule_ticks = crate::time::get_ticks();
        scheduler::RESCHED_REQUESTED[cpu].store(false, Ordering::Relaxed);
        self.resolve_pending_next_locked();
        apply_isr_wakeups(&mut self.0);
        if crate::net::loopback_queue_has_work() {
            let pump = crate::net::loopback_pump_tid();
            if pump != 0 {
                match wake(&mut self.0, pump, Wake::Generic) {
                    Woken::Done(UnblockOutcome::Transitioned) => {
                        crate::net::record_loopback_pump_rearm_from_sched()
                    }
                    Woken::Busy => hold(pump, WAKE_GENERIC, 0),
                    Woken::Done(_) => {}
                }
            }
        }
        expire_timers(&mut self.0);
        reclaim_stalled(&mut self.0);

        let old = self.state().current_thread.unwrap_or(idle);
        let mut requeue = false;
        let mut holds_head = false;
        if old != idle {
            let queued = self.0.queue().contains(&old);
            if let Some(thread) = self.get_thread_mut(old) {
                if thread.state == ThreadState::Terminated {
                    if let Some(pid) = thread.owner_pid {
                        observe_exit_kick(pid);
                    }
                } else if !thread.state.is_blocked() {
                    thread.charge_cpu(crate::time::get_cpu_ticks());
                    thread.set_ready();
                    WAKE_SITE_SCHEDULE.fetch_add(1, Ordering::Relaxed);
                    record_ready_site(old, READY_SITE_SCHEDULE);
                    requeue = !queued;
                    if requeue {
                        ENQUEUE_DEFERRED.fetch_add(1, Ordering::Relaxed);
                        self.state().previous_thread = Some(old);
                    } else {
                        ENQUEUE_ALREADY_QUEUED_OK.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            holds_head = settle_outgoing(&mut self.0, old, requeue);
        }

        #[cfg(feature = "boot_tests")]
        let idle_inline = scheduler::IDLE_INLINE_SELECT[cpu].swap(false, Ordering::Relaxed);
        #[cfg(feature = "boot_tests")]
        if idle_inline {
            crate::task::strand_oracle::note_idle_inline_select();
        }
        #[cfg(all(feature = "boot_tests", not(feature = "strand_inject_live_outgoing")))]
        let inject = idle_inline;
        #[cfg(feature = "strand_inject_live_outgoing")]
        let inject = crate::task::ret_zero_pc_oracle::is_strand_live_driver(old);
        #[cfg(feature = "boot_tests")]
        let mut withheld = false;
        let mut skip = |_tid| {
            #[cfg(feature = "boot_tests")]
            if crate::task::strand_oracle::withhold_victim(_tid, inject) {
                withheld = true;
                return true;
            }
            false
        };
        let mut ignored_waiters = 0;
        let mut chosen = None;
        let mut keep_current = false;
        if POLICIES_IN_USE.load(Ordering::Acquire) {
            let outgoing_rank = if requeue { rank(&mut self.0, old) } else { 0 };
            let floor = best_queued_rank(&mut self.0, None).max(outgoing_rank);
            chosen = steal_higher_ranked(&mut self.0, floor, &mut ignored_waiters);
            keep_current =
                chosen.is_none() && requeue && policy_keeps_cpu(&mut self.0, old, holds_head);
        }
        if chosen.is_none() && !keep_current {
            chosen = pop_local(&mut self.0, &mut ignored_waiters, &mut skip).or_else(|| {
                (0..online())
                    .filter(|&peer| peer != cpu && (!requeue || !is_idle(peer)))
                    .find_map(|peer| {
                        steal_from(&mut self.0, peer, &mut ignored_waiters, false, &mut skip)
                    })
            });
        }

        if chosen == Some(old) {
            requeue = true;
            chosen = pop_local(&mut self.0, &mut ignored_waiters, &mut skip).or_else(|| {
                (0..online()).filter(|&peer| peer != cpu).find_map(|peer| {
                    steal_from(&mut self.0, peer, &mut ignored_waiters, true, &mut skip)
                })
            });
        }
        let next = chosen.unwrap_or(idle);
        #[cfg(feature = "boot_tests")]
        if withheld && old == idle && next == idle {
            scheduler::set_need_resched();
        }
        let current_can_stay = requeue
            && self.get_thread(old).is_some_and(|thread| {
                thread.privilege == ThreadPrivilege::User
                    && thread.cpu_affinity.map_or(true, |pin| pin.cpu == cpu)
            });
        if keep_current || (next == idle && (old == idle || current_can_stay)) {
            self.state().previous_thread = None;
            if let Some(thread) = self.get_thread_mut(old) {
                thread.set_running();
                thread.sched.head_on_requeue = false;
            }
            set_cpu_idle(cpu, old == idle);
            return None;
        }

        if let Some(thread) = self.get_thread_mut(next) {
            Scheduler::set_selected_running(thread, cpu);
        }
        let state = self.state();
        state.pending_next = Some(next);
        state.last_dispatch_promoted = state.promoted_wake == Some(next);
        if state.last_dispatch_promoted {
            state.promoted_wake = None;
        }
        set_cpu_idle(cpu, next == idle);
        #[cfg(feature = "coreproof_component_h")]
        crate::proof_point!(IncomingHandoffCommit);
        #[cfg(feature = "coreproof_component_a")]
        crate::proof_point!(DeferredRequeueClaim);
        Some((old, next, requeue))
    }
}

use crate::tracing::counter::{register_counter, TraceCounter};

static ACQUIRE_TICKS: [TraceCounter; 3] = [
    TraceCounter::new(
        "ARM64_RQ_ACQUIRE_LT_512",
        "Run-queue acquisition below 512 counter ticks",
    ),
    TraceCounter::new(
        "ARM64_RQ_ACQUIRE_LT_4096",
        "Run-queue acquisition from 512 to 4095 counter ticks",
    ),
    TraceCounter::new(
        "ARM64_RQ_ACQUIRE_GE_4096",
        "Run-queue acquisition at least 4096 counter ticks",
    ),
];

static HOLD_TICKS: [TraceCounter; 3] = [
    TraceCounter::new(
        "ARM64_RQ_HOLD_LT_512",
        "Run-queue hold below 512 counter ticks",
    ),
    TraceCounter::new(
        "ARM64_RQ_HOLD_LT_4096",
        "Run-queue hold from 512 to 4095 counter ticks",
    ),
    TraceCounter::new(
        "ARM64_RQ_HOLD_GE_4096",
        "Run-queue hold at least 4096 counter ticks",
    ),
];

pub(super) fn register_lock_intervals() {
    for counter in ACQUIRE_TICKS.iter().chain(HOLD_TICKS.iter()) {
        register_counter(counter);
    }
}

pub(super) fn note_lock_interval(acquire: bool, ticks: u64) {
    let bucket = usize::from(ticks >= 512) + usize::from(ticks >= 4096);
    let bins = if acquire { &ACQUIRE_TICKS } else { &HOLD_TICKS };
    bins[bucket].increment();
}
