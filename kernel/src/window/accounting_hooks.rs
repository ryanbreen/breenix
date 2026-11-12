//! Kernel identities and clocks for the numeric accounting journal.
use super::Accounting;
use crate::task::thread::{Thread, ThreadState};
use core::sync::atomic::Ordering::Relaxed;

static TRACE: Accounting<32768, 1024, 4096> = Accounting::new();
fn now() -> u64 {
    crate::signal::monotonic_nanos()
}

pub fn thread_state(thread: &Thread, state: ThreadState) {
    let (mode, fallback) = match state {
        ThreadState::Ready => (0, 0),
        ThreadState::Running => (1, 0),
        ThreadState::Terminated => (3, 0),
        ThreadState::BlockedOnTimer => (2, 6),
        ThreadState::BlockedOnSignal => (2, 7),
        ThreadState::BlockedOnChildExit => (2, 5),
        _ => (2, 8),
    };
    let signals = &thread.signals;
    let window = TRACE.active_window();
    if window != 0 && signals.diagnostic_accounting_window.swap(window, Relaxed) != window {
        let group = signals.diagnostic_group.load(Relaxed);
        TRACE.identity(
            now(),
            thread.owner_pid.unwrap_or(0),
            thread.id(),
            if group == 0 {
                thread.owner_pid.unwrap_or(0)
            } else {
                group
            },
            &thread.name,
        );
    }
    let kind = signals.diagnostic_wait_kind.load(Relaxed);
    let ns = now();
    TRACE.state(
        ns,
        thread.owner_pid.unwrap_or(0),
        thread.id(),
        mode,
        if mode == 2 {
            if kind == 0 {
                fallback
            } else {
                kind
            }
        } else {
            0
        },
        if mode == 2 {
            signals.diagnostic_wait_object.load(Relaxed)
        } else {
            0
        },
    );
    if matches!(state, ThreadState::Ready | ThreadState::Running) && thread.state.is_blocked() {
        TRACE.disk_stage(signals.diagnostic_disk_request.load(Relaxed), 4, ns);
    }
}
pub fn active() -> bool {
    TRACE.is_active()
}
pub fn queue_on(cpu: u64, len: u64) {
    TRACE.queue(now(), cpu, len);
}
pub fn queue(len: u64) {
    TRACE.queue(
        now(),
        u64::MAX,
        len,
    );
}
pub fn exec(pid: u64) {
    #[cfg(target_arch = "aarch64")]
    let cpus = crate::arch_impl::aarch64::smp::cpus_online();
    #[cfg(target_arch = "x86_64")]
    let cpus = crate::arch_impl::x86_64::smp::cpus_online();
    TRACE.begin(now(), pid, cpus);
    if let Some(thread) = crate::task::thread::current_accounting_thread() {
        thread.signals.diagnostic_group.store(pid, Relaxed);
        // Exec commit leaves the scheduler row Ready, but this is the CPU's
        // current owner returning into its new image, not a queued contender.
        thread_state(thread, ThreadState::Running);
    }
}
pub fn composite(pid: u64) {
    TRACE.end(now(), pid);
}
pub fn file_syscall(nr: u64, entry: bool, result: i64) {
    #[cfg(target_arch = "aarch64")]
    let file = matches!(nr, 56 | 57 | 63 | 67 | 79 | 80 | 291);
    #[cfg(target_arch = "x86_64")]
    let file = matches!(nr, 0 | 2 | 3 | 4 | 5 | 6 | 17 | 257 | 262 | 332);
    if file {
        if let Some((pid, tid)) = crate::task::thread::current_accounting_identity() {
            TRACE.syscall(now(), pid, tid, nr, entry, result);
        }
    }
}
pub struct DiskScope {
    id: u64,
    previous: u64,
    signals: Option<&'static crate::signal::ThreadSignals>,
}
impl DiskScope {
    pub fn id(&self) -> u64 {
        self.id
    }
}
impl Drop for DiskScope {
    fn drop(&mut self) {
        if let Some(signals) = self.signals {
            signals
                .diagnostic_disk_request
                .store(self.previous, Relaxed);
        }
    }
}
pub fn disk_enqueue(device: u64, op: u64, sector: u64, bytes: u64) -> DiskScope {
    let (pid, tid) = crate::task::thread::current_accounting_identity().unwrap_or((0, 0));
    let id = TRACE.disk_enqueue(now(), pid, tid, device, op, sector, bytes);
    let signals = crate::task::thread::current_thread_signals();
    let previous = signals.map_or(0, |s| s.diagnostic_disk_request.swap(id, Relaxed));
    DiskScope {
        id,
        previous,
        signals,
    }
}
pub fn current_request() -> u64 {
    crate::task::thread::current_thread_signals()
        .map_or(0, |s| s.diagnostic_disk_request.load(Relaxed))
}
pub fn disk_stage(id: u64, stage: usize) {
    TRACE.disk_stage(id, stage, now());
}
pub fn text() -> alloc::string::String {
    TRACE.text(now())
}

/// Called only at a real outgoing dispatch trace, including idle switches.
pub fn switch_out(tid: u64) {
    TRACE.switch_out(now(), tid);
}
pub fn disk_irq_entry(device: u64) {
    let ns = now();
    let cpu = crate::task::scheduler::current_cpu_id_raw() as u64;
    TRACE.disk_irq_entry(ns, device, cpu);
}
pub fn wake(kind: u64, object: u64, target: u64, count: u64) {
    if TRACE.is_active() {
        let ns = now();
        let (pid, tid) = crate::task::thread::current_accounting_identity().unwrap_or((0, 0));
        TRACE.wake(ns, pid, tid, kind, object, target, count);
    }
}

pub fn join(word: u64, target_tid: u64, target_pid: u64) {
    if TRACE.is_active() {
        let (pid, tid) = crate::task::thread::current_accounting_identity().unwrap_or((0, 0));
        TRACE.join(now(), pid, tid, word, target_tid, target_pid);
    }
}

/// Input observation controls arrive through the existing root-only evidence device.
pub fn input_command(pid: u64, command: &str) {
    let mut words = command.split_whitespace();
    match words.next() {
        Some("input-reset") => TRACE.reset_input(),
        Some("input-close") => TRACE.close_input(),
        Some("input-begin") => {
            if let Some(ns) = words.next().and_then(|w| w.parse().ok()) {
                #[cfg(target_arch = "aarch64")]
                let cpus = crate::arch_impl::aarch64::smp::cpus_online();
                #[cfg(target_arch = "x86_64")]
                let cpus = crate::arch_impl::x86_64::smp::cpus_online();
                TRACE.begin_input(ns, pid, cpus);
            }
        }
        Some("input-end") => {
            if let Some(ns) = words.next().and_then(|w| w.parse().ok()) { TRACE.finish_input(ns); }
        }
        _ => (),
    }
}

fn phase_code(name: &str) -> u64 {
    (1..=18).find(|&i| super::acquire_phase_name(i) == name).unwrap_or(0)
}
fn phase_at(phase: u64, edge: u64, object: u64) {
    if !TRACE.is_active() { return; }
    let ns = now();
    let (pid, tid) = crate::task::thread::current_accounting_identity().unwrap_or((0, 0));
    let cpu = crate::task::thread::current_thread_cpu_split_ns()
        .map_or(u64::MAX, |(user, system)| user.saturating_add(system));
    TRACE.acquire_phase(ns, pid, tid, phase, edge, object, cpu);
}
/// RAII closes error paths too. Only fixed numeric journal writes occur on the path.
pub struct AcquirePhase { phase: u64, object: u64 }
impl AcquirePhase {
    pub fn new(name: &str, object: u64) -> Self {
        let phase = phase_code(name);
        phase_at(phase, 1, object);
        Self { phase, object }
    }
}
impl Drop for AcquirePhase {
    fn drop(&mut self) { phase_at(self.phase, 0, self.object); }
}
/// A predicate refusal names the dependency it found, without changing its decision.
pub fn acquire_dependency(name: &str, object: u64) {
    phase_at(phase_code(name), 2, object);
}
