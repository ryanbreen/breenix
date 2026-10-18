//! Ruling S26: deferred reclamation progresses while a spinning thread holds
//! each CPU.
//!
//! # The leg
//!
//! A CLONE_VM-only child's receipt names no page-table root of its own (the
//! child runs on its parent's root, `inherited_cr3`), so once its row is gone
//! only the receipt's two-epoch retirement fence stands between it and
//! `record_reclaim`. The fence is still open when the exit's own drain runs,
//! so the receipt waits for a later drain. Before the reclaim daemon, the
//! later drains ran only when a CPU went idle or a process was created,
//! exec'd or exited; #1923's one-vCPU SIGKILL teardown wait did not end because
//! a spinning sibling kept the CPU from idling.
//!
//! This leg builds that receipt while each CPU is busy and requires the daemon
//! to retire it within `BOUND_MS`:
//!
//! * One CPU-bound kernel thread is pinned to each online CPU and spins
//!   without yielding for the whole window, so no CPU's run queue empties and
//!   no idle thread is selected.
//! * While the window is open, drains other than the daemon's refuse their
//!   passes (`boot_set_reclaim_daemon_only`). That stands in for the absence of
//!   process lifecycle events deterministically: with it, a kernel without
//!   the daemon leaves the receipt queued until the backstop, whatever the
//!   placement does.
//! * The leader and its CLONE_VM-only member are real process rows, and the
//!   member is retired through the exit path's own calls
//!   (`defer_process_resources`, `release_process_resources`, the row's
//!   removal and `enqueue_process_reclaim`).
//! * The drain's retire arm records whether the daemon or another drain
//!   retired the watched pid; the verdict requires the daemon.
//!
//! # The bound
//!
//! The daemon sleeps `RECLAIM_DAEMON_IDLE_NS` (100 ms) while both reclaim
//! queues are empty and `RECLAIM_DAEMON_RETRY_NS` (10 ms) while a receipt
//! waits. A timer wake is enqueued at the head of its CPU's queue, so the
//! daemon runs within the running spinner's remaining quantum plus a tick. A
//! receipt queued just after the daemon went to sleep therefore waits at most
//! the idle interval and a dispatch, then possibly one retry and a dispatch for
//! the fence: `mechanism_ms` on the marker is that sum. `BOUND_MS` is larger
//! because the gates run the guest under emulation, where timers can be
//! delivered late; the extra is a jitter allowance, not part of the mechanism
//! claim.
//!
//! # Not claimed
//!
//! * The window has no user-mode spinner: the spinners are kernel threads,
//!   which hold a CPU the same way for the scheduler.
//! * It does not measure throughput of a large backlog, only that one receipt
//!   is retired within the bound.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::serial_println;
use crate::task::kthread::{
    kthread_join, kthread_park, kthread_run, kthread_run_on_cpu, kthread_unpark,
};
use crate::task::process_task::{self, BootReclaimWatch};
use crate::task::scheduler;
use crate::{arch_disable_interrupts, arch_enable_interrupts, arch_halt_with_interrupts};

/// Architecture tag on the marker, so one architecture's gate cannot be
/// satisfied by the other's line.
const ARCH_TAG: &str = if cfg!(target_arch = "x86_64") {
    "x86"
} else {
    "aarch64"
};

/// One dispatch behind a spinner: its remaining quantum plus a tick.
const DISPATCH_MS: u64 =
    crate::task::timer_wake_oracle::QUANTUM_MS + crate::time::timer::MS_PER_TICK;
/// The daemon's sleeps, in milliseconds.
const DAEMON_IDLE_MS: u64 = process_task::RECLAIM_DAEMON_IDLE_NS / 1_000_000;
const DAEMON_RETRY_MS: u64 = process_task::RECLAIM_DAEMON_RETRY_NS / 1_000_000;
/// Mechanism bound: the idle sleep and a dispatch, then a retry and a dispatch.
const MECHANISM_MS: u64 = DAEMON_IDLE_MS + DISPATCH_MS + DAEMON_RETRY_MS + DISPATCH_MS;
/// Gate bound; the remainder over `MECHANISM_MS` is an emulation allowance.
const BOUND_MS: u64 = 1_000;

/// Backstops: "the boot is broken" budgets. Each that fires is counted on the
/// marker.
const QUIESCE_BACKSTOP_MS: u64 = 30_000;
const START_BACKSTOP_MS: u64 = 60_000;
const RETIRE_BACKSTOP_MS: u64 = 30_000;
const SPIN_BACKSTOP_MS: u64 = 120_000;

static OPEN: AtomicBool = AtomicBool::new(false);
static RUN: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicU32 = AtomicU32::new(0);
static SPINNING: AtomicU32 = AtomicU32::new(0);
static BACKSTOPS: AtomicU32 = AtomicU32::new(0);
static SPINS: [AtomicU64; scheduler::MAX_CPUS] = [const { AtomicU64::new(0) }; scheduler::MAX_CPUS];

fn now_ns() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

/// Halt until `ready()` or the budget ends; a halt keeps the caller runnable,
/// so it neither blocks nor drains. False, counted as a backstop, if the
/// budget ended the wait.
fn wait_until<F: Fn() -> bool>(ready: F, budget_ms: u64) -> bool {
    let deadline = now_ns().saturating_add(budget_ms.saturating_mul(1_000_000));
    loop {
        if ready() {
            return true;
        }
        if now_ns() >= deadline {
            BACKSTOPS.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        arch_halt_with_interrupts();
    }
}

/// A CPU-bound thread pinned to `cpu`: parked until the window opens, then
/// spinning without yielding until it closes.
fn spinner_body(cpu: usize) {
    unsafe { arch_enable_interrupts() };
    STARTED.fetch_add(1, Ordering::AcqRel);
    while !OPEN.load(Ordering::Acquire) {
        kthread_park();
    }
    SPINNING.fetch_add(1, Ordering::AcqRel);
    let backstop = now_ns().saturating_add(SPIN_BACKSTOP_MS * 1_000_000);
    while RUN.load(Ordering::Acquire) {
        if now_ns() >= backstop {
            BACKSTOPS.fetch_add(1, Ordering::Relaxed);
            break;
        }
        SPINS[cpu].fetch_add(1, Ordering::Relaxed);
        for _ in 0..256 {
            core::hint::spin_loop();
        }
    }
}

/// Retire `pid`'s row the way an exit's defer arm does and queue its receipt.
fn retire_through_exit_path(pid: crate::process::ProcessId) -> Result<(), &'static str> {
    // Removed rows are dropped after the process-manager guard.
    let mut removed = Vec::new();
    let reclaim = {
        let mut manager_guard = crate::process::manager();
        let manager = manager_guard.as_mut().ok_or("no process manager")?;
        let process = manager.get_process_mut(pid).ok_or("row missing")?;
        let reclaim = process_task::defer_process_resources(process);
        process_task::release_process_resources(process);
        manager.remove_from_ready_queue(pid);
        removed.push(manager.remove_process(pid));
        reclaim
    };
    removed.clear();
    process_task::enqueue_process_reclaim(reclaim);
    Ok(())
}

/// The leader's root and address-space lifetime.
fn address_space_of(
    pid: crate::process::ProcessId,
) -> Option<(
    u64,
    alloc::sync::Arc<crate::process::process::AddressSpaceLifetime>,
)> {
    let manager_guard = crate::process::manager();
    let row = manager_guard.as_ref()?.get_process(pid)?;
    Some((
        row.page_table
            .as_ref()?
            .level_4_frame()
            .start_address()
            .as_u64(),
        row.address_space_lifetime.clone()?,
    ))
}

/// A CLONE_VM-only child of the leader, as `clone` builds one without
/// CLONE_THREAD: on the leader's root, sharing its lifetime, in no thread group.
fn insert_clone_vm_member(
    leader_root: u64,
    lifetime: &alloc::sync::Arc<crate::process::process::AddressSpaceLifetime>,
) -> Option<crate::process::ProcessId> {
    #[cfg(not(target_arch = "x86_64"))]
    use crate::memory::arch_stub::VirtAddr;
    #[cfg(target_arch = "x86_64")]
    use x86_64::VirtAddr;

    fn member_entry() {}

    let mut manager_guard = crate::process::manager();
    let manager = manager_guard.as_mut()?;
    let pid = manager.allocate_pid();
    let name = "reclaim_busy_member";
    let mut process = crate::process::Process::new(
        pid,
        alloc::string::String::from(name),
        VirtAddr::new(0x0040_0000),
    );
    let mut thread = crate::task::thread::Thread::new(
        alloc::string::String::from(name),
        member_entry,
        VirtAddr::new(0x0080_0000),
        VirtAddr::new(0x007f_0000),
        VirtAddr::new(0x0001_0000),
        crate::task::thread::ThreadPrivilege::Kernel,
    );
    thread.owner_pid = Some(pid.as_u64());
    process.set_main_thread(thread);
    process.inherited_cr3 = Some(leader_root);
    process.address_space_lifetime = Some(lifetime.clone());
    manager.insert_process(pid, process);
    Some(pid)
}

struct Verdict {
    cpus: u32,
    spawned: usize,
    progressed: u32,
    retired_by: BootReclaimWatch,
    latency_ms: u64,
    daemon_passes: u64,
    cleaned: bool,
    reason: &'static str,
}

fn emit(verdict: &Verdict) {
    let started = STARTED.load(Ordering::Acquire);
    let spinning = SPINNING.load(Ordering::Acquire);
    let backstops = BACKSTOPS.load(Ordering::Relaxed);
    let cpus = verdict.cpus;
    let pass = verdict.reason.is_empty()
        && cpus >= 1
        && verdict.spawned == cpus as usize
        && started == cpus
        && spinning == cpus
        && verdict.progressed == cpus
        && verdict.retired_by == BootReclaimWatch::ByDaemon
        && verdict.latency_ms <= BOUND_MS
        && verdict.cleaned
        && backstops == 0;
    let retired_by = match verdict.retired_by {
        BootReclaimWatch::ByDaemon => "daemon",
        BootReclaimWatch::ByOtherDrain => "other",
        BootReclaimWatch::Pending => "none",
    };
    serial_println!(
        "[RECLAIM_BUSY_CPU_ORACLE:{}:cpus={}:spinners={}:started={}:spinning={}:progressed={}:retired_by={}:latency_ms={}:bound_ms={}:mechanism_ms={}:daemon_passes={}:cleaned={}:backstops={}:{}{}]",
        ARCH_TAG,
        cpus,
        verdict.spawned,
        started,
        spinning,
        verdict.progressed,
        retired_by,
        verdict.latency_ms,
        BOUND_MS,
        MECHANISM_MS,
        verdict.daemon_passes,
        u8::from(verdict.cleaned),
        backstops,
        if pass { "PASS" } else { "FAIL" },
        verdict.reason,
    );
}

/// Run the leg once and print `[RECLAIM_BUSY_CPU_ORACLE:...]`. Restores the
/// interrupt flag it found.
pub fn run() {
    let interrupts_were_enabled = crate::arch_interrupts_enabled();
    if !interrupts_were_enabled {
        unsafe { arch_enable_interrupts() };
    }
    // As in `timer_wake_oracle`: the boot caller may be the idle task, which a
    // spinner would keep off its CPU; a kernel thread coordinates instead.
    match kthread_run(run_coordinator, "s26_coord") {
        Ok(handle) => {
            if kthread_join(&handle).is_err() {
                serial_println!(
                    "[RECLAIM_BUSY_CPU_ORACLE:{}:FAIL:reason=coordinator_join_failed]",
                    ARCH_TAG
                );
            }
        }
        Err(_) => serial_println!(
            "[RECLAIM_BUSY_CPU_ORACLE:{}:FAIL:reason=coordinator_spawn_failed]",
            ARCH_TAG
        ),
    }
    if !interrupts_were_enabled {
        unsafe { arch_disable_interrupts() };
    }
}

fn run_coordinator() {
    unsafe { arch_enable_interrupts() };
    OPEN.store(false, Ordering::Release);
    RUN.store(true, Ordering::Release);
    STARTED.store(0, Ordering::Release);
    SPINNING.store(0, Ordering::Release);
    BACKSTOPS.store(0, Ordering::Release);
    for spins in &SPINS {
        spins.store(0, Ordering::Relaxed);
    }

    let online_mask = scheduler::RetirementSnapshot::capture().online_mask;
    let mut verdict = Verdict {
        cpus: online_mask.count_ones(),
        spawned: 0,
        progressed: 0,
        retired_by: BootReclaimWatch::Pending,
        latency_ms: 0,
        daemon_passes: 0,
        cleaned: false,
        reason: "",
    };
    if !process_task::boot_reclaim_daemon_started() {
        verdict.reason = ":reason=no_daemon";
        emit(&verdict);
        return;
    }
    // Earlier boot work may still have receipts in flight; the leg starts on
    // empty queues so the watched receipt is the only one.
    wait_until(
        || process_task::boot_reclaim_queue_census() == (0, 0),
        QUIESCE_BACKSTOP_MS,
    );

    #[cfg(target_arch = "aarch64")]
    let image = crate::memory::process_memory::valid_executable_fixture();
    #[cfg(target_arch = "x86_64")]
    let image = crate::memory::process_memory::x86_valid_executable_fixture();
    let leader = {
        let mut manager_guard = crate::process::manager();
        manager_guard.as_mut().and_then(|manager| {
            manager
                .create_process(alloc::string::String::from("reclaim_busy_leader"), &image)
                .ok()
        })
    };
    let Some(leader) = leader else {
        verdict.reason = ":reason=leader_create_failed";
        emit(&verdict);
        return;
    };
    let member = address_space_of(leader)
        .and_then(|(root, lifetime)| insert_clone_vm_member(root, &lifetime));
    let Some(member) = member else {
        let _ = retire_through_exit_path(leader);
        verdict.reason = ":reason=member_create_failed";
        emit(&verdict);
        return;
    };

    // One spinner per online CPU, parked until each has started so the
    // creations are not slowed by spinners already holding CPUs.
    let mut spinners = Vec::new();
    for cpu in 0..scheduler::MAX_CPUS {
        if online_mask & (1 << cpu) == 0 {
            continue;
        }
        match kthread_run_on_cpu(move || spinner_body(cpu), "s26_spin", cpu) {
            Ok(handle) => spinners.push(handle),
            Err(_) => break,
        }
    }
    verdict.spawned = spinners.len();
    wait_until(
        || STARTED.load(Ordering::Acquire) as usize >= spinners.len(),
        START_BACKSTOP_MS,
    );
    OPEN.store(true, Ordering::Release);
    // Repeat the unparks until each spinner reports itself spinning: one that
    // parked after a single unpark would otherwise stay parked.
    let deadline = now_ns().saturating_add(START_BACKSTOP_MS * 1_000_000);
    while (SPINNING.load(Ordering::Acquire) as usize) < spinners.len() {
        for handle in &spinners {
            kthread_unpark(handle);
        }
        if now_ns() >= deadline {
            BACKSTOPS.fetch_add(1, Ordering::Relaxed);
            break;
        }
        arch_halt_with_interrupts();
    }

    // The window: a spinner on each CPU, only the daemon may drain.
    process_task::boot_set_reclaim_daemon_only(true);
    process_task::boot_reclaim_watch(member.as_u64());
    let spins_before: [u64; scheduler::MAX_CPUS] =
        core::array::from_fn(|cpu| SPINS[cpu].load(Ordering::Relaxed));
    let passes_before = process_task::boot_reclaim_daemon_passes();
    let retired_at_start = now_ns();
    let member_retired = retire_through_exit_path(member);
    if member_retired.is_ok() {
        wait_until(
            || process_task::boot_reclaim_watch_state() != BootReclaimWatch::Pending,
            RETIRE_BACKSTOP_MS,
        );
    }
    let retired_at = now_ns();
    verdict.retired_by = process_task::boot_reclaim_watch_state();
    verdict.latency_ms = retired_at.saturating_sub(retired_at_start) / 1_000_000;
    verdict.daemon_passes = process_task::boot_reclaim_daemon_passes().wrapping_sub(passes_before);
    verdict.progressed = (0..scheduler::MAX_CPUS)
        .filter(|&cpu| online_mask & (1 << cpu) != 0)
        .filter(|&cpu| SPINS[cpu].load(Ordering::Relaxed) > spins_before[cpu])
        .count() as u32;
    process_task::boot_set_reclaim_daemon_only(false);
    process_task::boot_reclaim_watch(0);

    RUN.store(false, Ordering::Release);
    for handle in &spinners {
        let _ = kthread_join(handle);
    }

    // Leave both receipts retired for the boot tests that follow: the
    // leader's receipt, which holds its root, retires once the member's row is
    // gone.
    let leader_retired = retire_through_exit_path(leader);
    verdict.cleaned = member_retired.is_ok()
        && leader_retired.is_ok()
        && wait_until(
            || {
                process_task::boot_reclaim_locations(member.as_u64()) == (false, false)
                    && process_task::boot_reclaim_locations(leader.as_u64()) == (false, false)
            },
            QUIESCE_BACKSTOP_MS,
        );
    if member_retired.is_err() {
        verdict.reason = ":reason=member_retire_failed";
    }
    emit(&verdict);
}
