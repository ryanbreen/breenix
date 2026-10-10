//! Signal-related data structures

use super::constants::*;
use crate::memory::slab::{SlabBox, SIGNAL_HANDLERS_SLAB};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Alternate signal stack configuration (matches Linux stack_t)
///
/// This structure represents the alternate signal stack that can be
/// configured per-process for handling signals like SIGSEGV that may
/// occur due to stack overflow.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct StackT {
    /// Base address of the alternate stack
    pub ss_sp: u64,
    /// Flags (SS_ONSTACK, SS_DISABLE)
    pub ss_flags: i32,
    /// Padding for alignment
    pub _pad: i32,
    /// Size of the alternate stack in bytes
    pub ss_size: usize,
}

impl Default for StackT {
    fn default() -> Self {
        StackT {
            ss_sp: 0,
            ss_flags: SS_DISABLE as i32,
            _pad: 0,
            ss_size: 0,
        }
    }
}

/// Per-process alternate signal stack state: the configured stack. Whether a
/// thread is running on it is a property of its stack pointer (`on_stack`),
/// as on Linux, so a handler left by longjmp or a stack switch is not taken
/// for one still running there.
#[derive(Debug, Clone, Copy)]
pub struct AltStack {
    /// Base address of the alternate stack
    pub base: u64,
    /// Size of the alternate stack in bytes
    pub size: usize,
    /// Flags (SS_DISABLE if disabled)
    pub flags: u32,
}

impl Default for AltStack {
    fn default() -> Self {
        Self { base: 0, size: 0, flags: SS_DISABLE }
    }
}

impl AltStack {
    /// Whether user stack pointer `sp` is on this alternate stack. The stack
    /// grows down, so its top, `base + size`, is on it and `base` is not.
    pub fn on_stack(&self, sp: u64) -> bool {
        self.flags & SS_DISABLE == 0 && sp > self.base && sp - self.base <= self.size as u64
    }

    /// The `stack_t` sigaltstack and a handler's `uc_stack` report for a
    /// thread whose stack pointer is `sp`.
    pub fn stack_t(&self, sp: u64) -> StackT {
        StackT {
            ss_sp: self.base,
            ss_flags: if self.on_stack(sp) {
                SS_ONSTACK as i32
            } else if self.flags & SS_DISABLE != 0 {
                SS_DISABLE as i32
            } else {
                0
            },
            _pad: 0,
            ss_size: self.size,
        }
    }
}

/// Default action for a signal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalDefaultAction {
    /// Terminate the process
    Terminate,
    /// Ignore the signal
    Ignore,
    /// Terminate with core dump
    CoreDump,
    /// Stop (pause) the process
    Stop,
    /// Continue a stopped process
    Continue,
}

/// Get the default action for a signal
pub fn default_action(sig: u32) -> SignalDefaultAction {
    match sig {
        // Terminate
        SIGHUP | SIGINT | SIGKILL | SIGPIPE | SIGALRM | SIGTERM | SIGUSR1 | SIGUSR2 | SIGIO
        | SIGPWR | SIGSTKFLT => SignalDefaultAction::Terminate,

        // Core dump
        SIGQUIT | SIGILL | SIGTRAP | SIGABRT | SIGBUS | SIGFPE | SIGSEGV | SIGXCPU | SIGXFSZ
        | SIGSYS => SignalDefaultAction::CoreDump,

        // Ignore
        SIGCHLD | SIGURG | SIGWINCH => SignalDefaultAction::Ignore,

        // Stop
        SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU => SignalDefaultAction::Stop,

        // Continue
        SIGCONT => SignalDefaultAction::Continue,

        // Default for unknown/realtime signals
        _ => SignalDefaultAction::Terminate,
    }
}

// SIGCONT's resume effect is applied at generation by send_signal_to_process.
// Its default disposition has no remaining delivery work after that effect.
const DEFAULT_IGNORED_SIGNALS: u64 =
    sig_mask(SIGCHLD) | sig_mask(SIGURG) | sig_mask(SIGWINCH) | sig_mask(SIGCONT);

/// Signal handler configuration: the Linux ABI's `struct sigaction` for
/// rt_sigaction, on x86-64 and ARM64 alike (handler, flags, restorer, mask).
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct SignalAction {
    /// Handler address (SIG_DFL, SIG_IGN, or user function pointer)
    pub handler: u64,
    /// Flags (SA_RESTART, SA_SIGINFO, etc.)
    pub flags: u64,
    /// Restorer function for sigreturn (provided by libc or kernel)
    pub restorer: u64,
    /// Signals to block during handler execution
    pub mask: u64,
}

impl Default for SignalAction {
    fn default() -> Self {
        SignalAction {
            handler: SIG_DFL,
            flags: 0,
            restorer: 0,
            mask: 0,
        }
    }
}

impl SignalAction {
    /// Check if handler is the default action
    #[inline]
    pub fn is_default(&self) -> bool {
        self.handler == SIG_DFL
    }

    /// Check if handler ignores the signal
    #[inline]
    pub fn is_ignore(&self) -> bool {
        self.handler == SIG_IGN
    }

    /// Check if handler is a user function
    #[inline]
    pub fn is_handler(&self) -> bool {
        self.handler > SIG_IGN
    }
}

/// What a pending signal carries for its handler's `siginfo_t` besides its
/// number: the si_code, si_errno and the first 16 bytes of the Linux siginfo union,
/// which hold si_pid and si_uid then si_value or si_status, or si_addr.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct SigInfo {
    pub code: i32,
    /// si_errno: zero except where a sender gave one (sigqueue).
    pub errno: i32,
    /// Bytes 16..32 of the Linux `siginfo_t`.
    pub fields: [u64; 2],
    /// For a fault, what the handler's machine context reports about the
    /// exception besides si_addr: on x86-64 the vector (`trapno`) and error
    /// code (`err`), on ARM64 the ESR (its `esr_context` record). Zero for
    /// any other signal.
    pub trap: Trap,
}

/// The exception a fault signal came from, as its sigcontext reports it.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct Trap {
    /// x86-64 exception vector; unused on ARM64.
    pub number: u64,
    /// x86-64 error code, or the ARM64 ESR.
    pub error: u64,
}

impl SigInfo {
    /// A signal the kernel generated with no sender (SI_KERNEL).
    pub const fn kernel() -> Self {
        Self { code: SI_KERNEL, errno: 0, fields: [0, 0], trap: Trap { number: 0, error: 0 } }
    }

    /// A signal sent by a process: `code` SI_USER or SI_TKILL, with the
    /// sender's PID and real user ID.
    pub const fn sender(code: i32, pid: u32, uid: u32) -> Self {
        Self { fields: [pid as u64 | (uid as u64) << 32, 0], ..Self::with_code(code) }
    }

    /// A fault at `addr` (si_addr).
    pub const fn fault(code: i32, addr: u64) -> Self {
        Self { fields: [addr, 0], ..Self::with_code(code) }
    }

    /// This fault's siginfo, raised by exception `number` with error code
    /// (or ESR) `error`.
    pub const fn from_trap(self, number: u64, error: u64) -> Self {
        Self { trap: Trap { number, error }, ..self }
    }

    /// SIGCHLD for child `pid` of real user `uid`: `code` is a CLD_* value
    /// and `status` the exit status or the signal.
    pub const fn child(code: i32, pid: u32, uid: u32, status: i32) -> Self {
        Self {
            fields: [pid as u64 | (uid as u64) << 32, status as u32 as u64],
            ..Self::with_code(code)
        }
    }

    /// A POSIX timer's signal (SI_TIMER): the timer's ID, then its overrun
    /// count, which delivery fills in (`SignalState::take`), then sigev_value.
    pub const fn timer(id: i32, value: u64) -> Self {
        Self { fields: [id as u32 as u64, value], ..Self::with_code(super::timers::SI_TIMER) }
    }

    /// This timer siginfo with `overrun` as its si_overrun.
    fn with_overrun(self, overrun: i32) -> Self {
        Self { fields: [self.fields[0] & 0xffff_ffff | (overrun as u32 as u64) << 32, self.fields[1]], ..self }
    }

    const fn with_code(code: i32) -> Self {
        Self { code, ..Self::kernel() }
    }

    /// The `siginfo_t` for signal `sig` in the Linux ABI's 128-byte layout.
    pub fn to_linux(&self, sig: u32) -> LinuxSigInfo {
        let mut words = [0u64; 16];
        words[0] = sig as u64 | (self.errno as u32 as u64) << 32;
        words[1] = self.code as u32 as u64;
        words[2] = self.fields[0];
        words[3] = self.fields[1];
        LinuxSigInfo(words)
    }
}

/// A Linux `siginfo_t`: si_signo, si_errno and si_code as ints, then the
/// union from byte 16. 128 bytes on x86-64 and ARM64.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct LinuxSigInfo(pub [u64; 16]);

/// The si_code and si_status of the SIGCHLD a child's exit raises, from the
/// exit code the kernel records: a negative code is the signal that ended
/// it, with 0x80 for a core dump.
pub fn child_exit_code_status(exit_code: i32) -> (i32, i32) {
    if exit_code < 0 && (-exit_code) & 0x80 != 0 {
        (CLD_DUMPED, (-exit_code) & 0x7f)
    } else if exit_code < 0 {
        (CLD_KILLED, (-exit_code) & 0x7f)
    } else {
        (CLD_EXITED, exit_code & 0xff)
    }
}

/// A process's signal dispositions and the siginfo of each standard signal,
/// one per signal: a standard signal already pending keeps the information it
/// was first generated with, as on Linux. For a realtime signal the slot holds
/// the instance delivery last took off the queue (`SignalState::take`).
#[derive(Clone)]
pub struct SignalTable {
    actions: [SignalAction; 64],
    info: [SigInfo; 64],
}

impl SignalTable {
    const fn new() -> Self {
        SignalTable {
            actions: [SignalAction {
                handler: SIG_DFL,
                flags: 0,
                restorer: 0,
                mask: 0,
            }; 64],
            info: [SigInfo::kernel(); 64],
        }
    }
}

/// State owned by one thread, shared by its scheduler and process-table copies.
/// Dispositions remain under the process manager; these words never take its lock.
#[derive(Default)]
pub struct ThreadSignals {
    pub blocked: AtomicU64,
    saved_mask: AtomicU64,
    saved_mask_valid: AtomicBool,
    pub wait_set: AtomicU64,
    pub timer_pending: AtomicU64,
    /// Packed monotonic microseconds and user-mode bit; zero while not dispatched.
    pub cpu_clock: AtomicU64,
    pub in_user: AtomicBool,
    /// This thread's CPU time in user and in system mode, in nanoseconds:
    /// CLOCK_THREAD_CPUTIME_ID and getrusage(RUSAGE_THREAD).
    pub user_ns: AtomicU64,
    pub system_ns: AtomicU64,
    /// Signals of POSIX timers the scheduler expired with this thread as the
    /// recipient, not yet queued (`SignalState::collect_timer_signals`).
    pub posix_pending: AtomicU64,
    /// A POSIX timer signal is due here that the last signal check had no
    /// memory to queue; the next check tries again.
    pub posix_retry: AtomicBool,
}

impl ThreadSignals {
    pub fn with_mask(mask: u64) -> Self {
        Self {
            blocked: AtomicU64::new(mask),
            in_user: AtomicBool::new(true),
            ..Self::default()
        }
    }

    /// User and system nanoseconds charged to this thread so far.
    pub fn cpu_split_ns(&self) -> (u64, u64) {
        (self.user_ns.load(Ordering::Relaxed), self.system_ns.load(Ordering::Relaxed))
    }

    pub fn save_wait_mask(&self, mask: u64) {
        self.saved_mask.store(mask, Ordering::Relaxed);
        self.saved_mask_valid.store(true, Ordering::Release);
    }

    pub fn wait_mask(&self) -> Option<u64> {
        self.saved_mask_valid
            .load(Ordering::Acquire)
            .then(|| self.saved_mask.load(Ordering::Relaxed))
    }

    pub fn take_wait_mask(&self) -> Option<u64> {
        self.saved_mask_valid
            .swap(false, Ordering::AcqRel)
            .then(|| self.saved_mask.load(Ordering::Relaxed))
    }
}

/// Per-process signal state
///
/// Note: the dispositions and siginfo are boxed to avoid stack overflow. The
/// table is 3.5KB, which causes stack overflow during process creation if
/// stored inline.
#[derive(Clone)]
pub struct SignalState {
    /// Pending signals bitmap (signals waiting to be delivered)
    pub pending: u64,
    /// Which pending bits are process-directed and may move to another thread.
    pub process_pending: u64,
    /// The executing thread's masks and wait state, shared with its scheduler row.
    pub thread: Arc<ThreadSignals>,
    /// Signal handlers and pending siginfo (one each per signal, indices 0-63
    /// for signals 1-64). Slab-allocated for O(1) alloc/free, falls back to heap.
    handlers: SlabBox<SignalTable>,
    /// Cached disposition mask, maintained alongside the private handler table.
    ignored: u64,
    /// Alternate signal stack configuration
    pub alt_stack: AltStack,
    /// Signals whose SA_RESETHAND action delivery reset in this row and not
    /// yet in the other rows of its thread group (`mark_group_reset`).
    group_resets: u64,
    /// Pending instances of realtime signals, oldest first. A realtime
    /// signal's `pending` bit is set while it has one here, and its
    /// `process_pending` bit while one of them is process-directed.
    queued: Vec<Instance>,
}

/// One pending instance of a realtime signal, or of any signal a POSIX timer
/// sent.
#[derive(Clone)]
struct Instance {
    sig: u32,
    info: SigInfo,
    /// Its place in the order signals were generated (`NEXT_INSTANCE`), which
    /// keeps a signal's instances in that order when one moves to another
    /// thread of its process.
    seq: u64,
    /// Sent to the process, so it may move to another of its threads; else
    /// sent to this row's thread (tkill, tgkill), where it stays.
    process: bool,
    /// The POSIX timer that sent it: delivery reads its overrun count, and a
    /// discard tells the timer its signal is no longer pending.
    timer: Option<Arc<super::timers::TimerSignal>>,
}

/// A process-directed instance taken off one thread's queue to move to
/// another: its siginfo, generation order and timer.
pub type MovedSignal = (SigInfo, u64, Option<Arc<super::timers::TimerSignal>>);

/// The generation order of the next realtime instance.
static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);

/// Whether `sig` is a realtime signal, whose instances are queued rather than
/// coalesced.
pub const fn is_realtime(sig: u32) -> bool {
    sig >= SIGRTMIN && sig <= SIGRTMAX
}

/// Set when a row has group resets the process manager has not yet copied
/// to the rest of its thread group (`ProcessManager::finish_group_resets`).
/// Set and cleared with the process manager held.
pub static GROUP_RESETS_PENDING: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

impl Default for SignalState {
    fn default() -> Self {
        let handlers = if let Some(raw) = SIGNAL_HANDLERS_SLAB.alloc() {
            let table = raw as *mut SignalTable;
            unsafe {
                core::ptr::write(table, SignalTable::new());
                SlabBox::from_slab(table, &SIGNAL_HANDLERS_SLAB)
            }
        } else {
            SlabBox::from_box(alloc::boxed::Box::new(SignalTable::new()))
        };
        SignalState {
            pending: 0,
            process_pending: 0,
            thread: Arc::new(ThreadSignals::with_mask(0)),
            handlers,
            ignored: DEFAULT_IGNORED_SIGNALS,
            alt_stack: AltStack::default(),
            group_resets: 0,
            queued: Vec::new(),
        }
    }
}

impl SignalState {
    pub fn blocked(&self) -> u64 {
        self.thread.blocked.load(Ordering::Relaxed)
    }

    pub fn pending_set(&self) -> u64 {
        let expired = self.thread.timer_pending.load(Ordering::Acquire)
            | self.thread.posix_pending.load(Ordering::Acquire);
        self.pending | (expired & !self.ignored)
    }

    /// Queue the signals of the process's timers the scheduler expired with
    /// this row's thread as recipient: the interval timers' (`timers`, which
    /// is the process's), and the POSIX timers' due here, in expiry order.
    pub fn collect_timer_signals(&mut self, timers: &IntervalTimers) {
        let mut pending = self.thread.timer_pending.swap(0, Ordering::AcqRel);
        while pending != 0 {
            let sig = pending.trailing_zeros() + 1;
            pending &= !sig_mask(sig);
            self.set_process_pending(sig);
        }
        let due = self.thread.posix_pending.swap(0, Ordering::AcqRel) != 0;
        if due || self.thread.posix_retry.swap(false, Ordering::AcqRel) {
            loop {
                // Room first, so that a signal taken off its timer is always
                // queued; without memory it stays due for the next check.
                if !self.reserve_timer_signal() {
                    self.thread.posix_retry.store(true, Ordering::Release);
                    break;
                }
                let Some((sig, info, process, timer)) = timers.posix.take_next_due(&self.thread) else {
                    break;
                };
                self.queue_timer_signal(sig, info, process, timer);
            }
        }
    }

    /// Make room for the instances `queue_timer_signal` queues, so that it
    /// allocates nothing. False when there is no memory for them.
    pub fn reserve_timer_signal(&mut self) -> bool {
        self.queued.try_reserve(2).is_ok()
    }

    /// Queue a POSIX timer's signal, as an instance that holds the timer
    /// whatever the signal's number, so delivery can read its overrun count;
    /// `reserve_timer_signal` must have made room first. A signal discarded
    /// here is given back to the timer, whose next expiry generates another.
    /// An ordinary standard signal of the same number already pending keeps
    /// its place ahead of it, as an instance of its own, so taking the
    /// timer's does not take it too.
    pub fn queue_timer_signal(&mut self, sig: u32, info: SigInfo, process: bool, timer: Arc<super::timers::TimerSignal>) {
        if self.discards(sig) {
            timer.discard();
            return;
        }
        let bit = sig_mask(sig);
        self.queue_ordinary_pending(sig);
        let seq = NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed);
        self.queued.push(Instance { sig, info, seq, process, timer: Some(timer) });
        self.pending |= bit;
        if process {
            self.process_pending |= bit;
        }
    }

    /// Before an instance of standard signal `sig` is queued: an ordinary
    /// `sig` already pending, which has no instance, gets one, older than the
    /// one about to be queued, so each is taken in turn and the pending bit
    /// stays until both are. Room for it must have been reserved.
    fn queue_ordinary_pending(&mut self, sig: u32) {
        let bit = sig_mask(sig);
        if is_realtime(sig) || self.pending & bit == 0 || self.queued.iter().any(|i| i.sig == sig) {
            return;
        }
        let seq = NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed);
        let info = self.handlers.info[(sig - 1) as usize];
        let process = self.process_pending & bit != 0;
        self.queued.push(Instance { sig, info, seq, process, timer: None });
    }

    /// Drop every queued signal a POSIX timer sent and every one due here:
    /// exec, which deletes the timers.
    pub fn drop_timer_signals(&mut self) {
        self.thread.posix_pending.store(0, Ordering::Release);
        self.thread.posix_retry.store(false, Ordering::Release);
        let senders = self.queued.iter().filter(|i| i.timer.is_some()).fold(0u64, |mask, i| mask | sig_mask(i.sig));
        self.drop_instances(|i| i.timer.is_some());
        let mut senders = senders;
        while senders != 0 {
            let sig = senders.trailing_zeros() + 1;
            let bit = sig_mask(sig);
            senders &= !bit;
            let (left, process) = self
                .queued
                .iter()
                .filter(|i| i.sig == sig)
                .fold((false, false), |(_, process), i| (true, process || i.process));
            if !left {
                self.pending &= !bit;
            }
            if !process {
                self.process_pending &= !bit;
            }
        }
    }

    /// Create a new signal state with default handlers
    #[allow(dead_code)] // Used by Default trait, part of public API
    pub fn new() -> Self {
        Self::default()
    }

    /// Pending, unblocked signals with an observable disposition.
    /// The cached mask makes this O(1), including on syscall/interrupt return.
    #[inline]
    pub fn has_deliverable_signals(&self) -> bool {
        (self.pending_set() & !self.blocked() & !self.ignored) != 0
    }

    /// Interruptible waits use the same disposition decision as delivery.
    #[inline]
    pub fn has_interrupting_signals(&self) -> bool {
        self.has_deliverable_signals()
    }

    /// Whether a restartable wait this state's next deliverable signal
    /// interrupts is resumed (SA_RESTART): it is unless that signal will run a
    /// handler installed without SA_RESTART.
    pub fn interruption_restarts(&self) -> bool {
        match self.next_deliverable_signal() {
            Some(sig) => {
                let action = self.get_handler(sig);
                !action.is_handler() || action.flags & SA_RESTART != 0
            }
            None => true,
        }
    }

    /// Get the next deliverable signal: a fault's signal first, then the
    /// lowest number.
    ///
    /// Returns None if no signals are pending and unblocked
    pub fn next_deliverable_signal(&self) -> Option<u32> {
        let mut deliverable = self.pending_set() & !self.blocked() & !self.ignored;
        if deliverable == 0 {
            return None;
        }
        if deliverable & SYNCHRONOUS_SIGNALS != 0 {
            deliverable &= SYNCHRONOUS_SIGNALS;
        }
        // Find lowest set bit (trailing zeros gives the bit position)
        let bit = deliverable.trailing_zeros();
        Some(bit + 1) // Signal numbers are 1-based
    }

    /// Queue a synchronous fault, with its siginfo, even when its
    /// disposition blocks or ignores it: then it is unblocked and its action
    /// reset to SIG_DFL, as Linux's force_sig_info does. The fault's
    /// information replaces whatever an earlier instance left pending.
    pub fn force_signal(&mut self, sig: u32, info: SigInfo) {
        if self.is_blocked(sig) || self.get_handler(sig).is_ignore() {
            self.unblock_signals(super::constants::sig_mask(sig));
            self.set_handler(sig, SignalAction::default());
            self.mark_group_reset(sig);
        }
        self.clear_pending(sig);
        self.set_pending_info(sig, info);
    }

    /// Mark a signal the kernel generated as pending (si_code SI_KERNEL).
    #[inline]
    pub fn set_pending(&mut self, sig: u32) {
        self.set_pending_info(sig, SigInfo::kernel());
    }

    /// Whether generating `sig` here discards it: it is ignored, unless it is
    /// blocked and ignoring is its default action, when it is kept for
    /// synchronous acceptance, as Linux does. Explicit SIG_IGN discards.
    pub fn discards(&self, sig: u32) -> bool {
        !is_valid_signal(sig)
            || self.ignored & sig_mask(sig) != 0
                && !(self.is_blocked(sig) && self.get_handler(sig).is_default())
    }

    /// Mark a signal as pending for this row's thread with the siginfo its
    /// handler is to be given (`generate`).
    #[inline]
    pub fn set_pending_info(&mut self, sig: u32, info: SigInfo) {
        self.generate(sig, info, false);
    }

    /// Mark a signal the kernel generated as pending for this row's process,
    /// which any of its threads may accept (si_code SI_KERNEL).
    #[inline]
    pub fn set_process_pending(&mut self, sig: u32) {
        self.generate(sig, SigInfo::kernel(), true);
    }

    /// Generate `sig` with `info` for this row's thread or, when `process`,
    /// for its process. A standard signal already pending is not queued
    /// again and keeps its first information; each instance of a realtime
    /// signal is queued after those already pending. Returns false,
    /// generating nothing, when there is no memory for a realtime instance.
    /// The caller bounds the queue (RLIMIT_SIGPENDING).
    pub fn try_generate(&mut self, sig: u32, info: SigInfo, process: bool) -> bool {
        if self.discards(sig) {
            return true;
        }
        let bit = sig_mask(sig);
        if is_realtime(sig) {
            if self.queued.try_reserve(1).is_err() {
                return false;
            }
            let seq = NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed);
            self.queued.push(Instance { sig, info, seq, process, timer: None });
        } else if self.pending & bit == 0 {
            self.handlers.info[(sig - 1) as usize] = info;
        }
        self.pending |= bit;
        if process {
            self.process_pending |= bit;
        }
        true
    }

    /// Generate `sig` as `try_generate` does, except that a realtime signal
    /// with no memory for its instance is made pending as `generate_unqueued`
    /// makes it.
    pub fn generate(&mut self, sig: u32, info: SigInfo, process: bool) {
        if !self.try_generate(sig, info, process) {
            self.generate_unqueued(sig, info, process);
        }
    }

    /// Make `sig` pending without queuing an instance of it, when none of it
    /// is pending, its siginfo kept as a standard signal's is: what Linux's
    /// kill does with a realtime signal it cannot queue.
    pub fn generate_unqueued(&mut self, sig: u32, info: SigInfo, process: bool) {
        let bit = sig_mask(sig);
        if self.discards(sig) || self.pending & bit != 0 {
            return;
        }
        self.handlers.info[(sig - 1) as usize] = info;
        self.pending |= bit;
        if process {
            self.process_pending |= bit;
        }
    }

    /// The siginfo delivery hands the handler of `sig`: a pending standard
    /// signal's, or the realtime instance `take` last took.
    pub fn pending_info(&self, sig: u32) -> SigInfo {
        if is_valid_signal(sig) {
            self.handlers.info[(sig - 1) as usize]
        } else {
            SigInfo::kernel()
        }
    }

    /// The siginfo of the instance of pending signal `sig` that `take` would
    /// take next, leaving it pending.
    pub fn next_info(&self, sig: u32) -> SigInfo {
        match self.queued.iter().find(|i| i.sig == sig) {
            Some(Instance { info, timer: Some(timer), .. }) => info.with_overrun(timer.overrun()),
            Some(instance) => instance.info,
            None => self.pending_info(sig),
        }
    }

    /// Take one instance of pending signal `sig`, the oldest, and return its
    /// siginfo, which `pending_info` also reports until the next take. A
    /// realtime signal with more instances queued stays pending.
    pub fn take(&mut self, sig: u32) -> SigInfo {
        if let Some(at) = self.queued.iter().position(|i| i.sig == sig) {
            let instance = self.queued.remove(at);
            let info = match instance.timer {
                Some(timer) => instance.info.with_overrun(timer.deliver()),
                None => instance.info,
            };
            self.handlers.info[(sig - 1) as usize] = info;
            self.settle(sig);
            return info;
        }
        self.clear_pending(sig);
        self.pending_info(sig)
    }

    /// Take the oldest process-directed instance of pending signal `sig`, for
    /// another thread of the process to accept (`accept_moved`), with its
    /// generation order and the timer that sent it, if one did. None,
    /// clearing `sig`'s `process_pending` bit, when only thread-directed
    /// instances of a realtime `sig` are queued.
    pub fn take_process_directed(&mut self, sig: u32) -> Option<MovedSignal> {
        if let Some(at) = self.queued.iter().position(|i| i.sig == sig && i.process) {
            let instance = self.queued.remove(at);
            self.settle(sig);
            return Some((instance.info, instance.seq, instance.timer));
        }
        if self.queued.iter().any(|i| i.sig == sig) {
            self.process_pending &= !sig_mask(sig);
            return None;
        }
        let info = self.pending_info(sig);
        self.clear_pending(sig);
        Some((info, 0, None))
    }

    /// Make room for what `accept_moved` will queue: an instance, and an
    /// ordinary pending standard signal it keeps ahead of it
    /// (`queue_ordinary_pending`), so that it allocates nothing. False when
    /// there is no memory for them.
    pub fn reserve_instance(&mut self) -> bool {
        self.queued.try_reserve(2).is_ok()
    }

    /// Accept process-directed `sig`, which `take_process_directed` took from
    /// another thread of this process, in its generation order `seq` among
    /// the instances queued here. `reserve_instance` must have made room
    /// first, so a realtime instance, or one a POSIX timer sent, is queued
    /// with its timer and never lost.
    pub fn accept_moved(&mut self, (info, seq, timer): MovedSignal, sig: u32) {
        if self.discards(sig) {
            if let Some(timer) = timer {
                timer.discard();
            }
            return;
        }
        if !is_realtime(sig) && timer.is_none() {
            self.generate(sig, info, true);
            return;
        }
        self.queue_ordinary_pending(sig);
        let at = self.queued.iter().position(|i| i.seq > seq).unwrap_or(self.queued.len());
        self.queued.insert(at, Instance { sig, info, seq, process: true, timer });
        self.pending |= sig_mask(sig);
        self.process_pending |= sig_mask(sig);
    }

    /// After an instance of `sig` was taken off the queue: `sig` stays
    /// pending while it has an instance queued, and process-directed while
    /// one of those is.
    fn settle(&mut self, sig: u32) {
        let mut remaining = self.queued.iter().filter(|i| i.sig == sig);
        match remaining.next() {
            None => self.clear_pending(sig),
            Some(first) => {
                if !first.process && !remaining.any(|i| i.process) {
                    self.process_pending &= !sig_mask(sig);
                }
            }
        }
    }

    /// How many realtime signal instances are queued, other than those a
    /// POSIX timer sent, which count against RLIMIT_SIGPENDING as the timer
    /// itself (`PosixTimers::count`).
    pub fn queued_count(&self) -> usize {
        self.queued.iter().filter(|i| i.timer.is_none() && is_realtime(i.sig)).count()
    }

    /// Discard every pending realtime signal and free the queue, for a row
    /// that has exited: what it held no longer counts against its user.
    pub fn release_queued(&mut self) {
        self.discard_pending(REALTIME_SIGNALS);
        self.drop_instances(|_| true);
        self.queued = Vec::new();
    }

    /// Discard every pending signal in `mask`, every queued instance included.
    #[inline]
    pub fn discard_pending(&mut self, mask: u64) {
        self.pending &= !mask;
        self.process_pending &= !mask;
        self.thread.timer_pending.fetch_and(!mask, Ordering::AcqRel);
        self.drop_instances(|i| sig_mask(i.sig) & mask != 0);
    }

    /// Remove the queued instances `doomed` selects, telling the timers of
    /// those a timer sent that their signals are no longer pending.
    fn drop_instances(&mut self, doomed: impl Fn(&Instance) -> bool) {
        if !self.queued.iter().any(&doomed) {
            return;
        }
        for instance in self.queued.iter().filter(|i| doomed(i)) {
            if let Some(timer) = &instance.timer {
                timer.discard();
            }
        }
        self.queued.retain(|i| !doomed(i));
    }

    /// Discard pending signal `sig`, every queued instance included.
    #[inline]
    pub fn clear_pending(&mut self, sig: u32) {
        if is_valid_signal(sig) {
            self.discard_pending(sig_mask(sig));
        }
    }

    /// Check if a signal is pending
    #[inline]
    #[allow(dead_code)] // Part of complete signal API, will be used for debugging/diagnostics
    pub fn is_pending(&self, sig: u32) -> bool {
        (self.pending & sig_mask(sig)) != 0
    }

    /// Check if a signal is blocked
    #[inline]
    #[allow(dead_code)] // Part of complete signal API, will be used for debugging/diagnostics
    pub fn is_blocked(&self, sig: u32) -> bool {
        (self.blocked() & sig_mask(sig)) != 0
    }

    /// Get handler for a signal
    ///
    /// Returns the default handler for invalid signal numbers
    pub fn get_handler(&self, sig: u32) -> &SignalAction {
        if sig == 0 || sig > NSIG {
            // Return a static default for invalid signals
            static DEFAULT: SignalAction = SignalAction {
                handler: SIG_DFL,
                flags: 0,
                restorer: 0,
                mask: 0,
            };
            &DEFAULT
        } else {
            &self.handlers.actions[(sig - 1) as usize]
        }
    }

    /// Set handler for a signal
    ///
    /// Does nothing for invalid signal numbers
    pub fn set_handler(&mut self, sig: u32, action: SignalAction) {
        if is_valid_signal(sig) && sig_mask(sig) & UNCATCHABLE_SIGNALS == 0 {
            let bit = sig_mask(sig);
            self.handlers.actions[(sig - 1) as usize] = action;
            if action.is_ignore() || (action.is_default() && DEFAULT_IGNORED_SIGNALS & bit != 0) {
                self.ignored |= bit;
                // POSIX 2.4.3: installing ignore discards a pending signal,
                // whether blocked or unblocked.
                self.pending &= !bit;
                self.process_pending &= !bit;
                self.drop_instances(|i| i.sig == sig);
            } else {
                self.ignored &= !bit;
            }
        }
    }

    /// Block additional signals
    #[inline]
    pub fn block_signals(&mut self, mask: u64) {
        // Cannot block SIGKILL or SIGSTOP
        self.thread
            .blocked
            .fetch_or(mask & !UNCATCHABLE_SIGNALS, Ordering::Relaxed);
    }

    /// Unblock signals
    #[inline]
    pub fn unblock_signals(&mut self, mask: u64) {
        self.thread.blocked.fetch_and(!mask, Ordering::Relaxed);
    }

    /// Set the blocked signal mask
    #[inline]
    pub fn set_blocked(&mut self, mask: u64) {
        // Cannot block SIGKILL or SIGSTOP
        self.thread
            .blocked
            .store(mask & !UNCATCHABLE_SIGNALS, Ordering::Relaxed);
    }

    /// Fork the signal state for a child process
    ///
    /// Pending signals are NOT inherited, but handlers, mask, and alt stack are
    #[allow(dead_code)] // Will be used when fork() implementation is complete
    pub fn fork(&self) -> Self {
        SignalState {
            pending: 0,
            process_pending: 0, // Child starts with no pending signals
            thread: Arc::new(ThreadSignals::with_mask(self.blocked())),
            handlers: self.handlers.clone(),
            ignored: self.ignored,
            alt_stack: self.alt_stack, // Alt stack is inherited per POSIX
            group_resets: 0,
            queued: Vec::new(),
        }
    }

    /// Record that delivery reset `sig`'s action to SIG_DFL in this row for
    /// SA_RESETHAND. Dispositions belong to the process, so the reset is
    /// copied to the group's other rows before the process manager is
    /// released (#1231). PM held.
    pub fn mark_group_reset(&mut self, sig: u32) {
        self.group_resets |= sig_mask(sig);
        GROUP_RESETS_PENDING.store(true, core::sync::atomic::Ordering::Relaxed);
    }

    /// The signals `mark_group_reset` recorded, cleared.
    pub fn take_group_resets(&mut self) -> u64 {
        core::mem::take(&mut self.group_resets)
    }

    /// Signal state for a new thread of this one's thread group: the creating
    /// thread's dispositions and mask, nothing pending and no alternate stack
    /// (POSIX pthread_create).
    pub fn new_thread(&self) -> Self {
        SignalState {
            alt_stack: AltStack::default(),
            ..self.fork()
        }
    }

    /// Reset signal handlers to default after exec
    ///
    /// Per POSIX, caught signals are reset to SIG_DFL, ignored signals stay ignored
    ///
    /// #721 m6: the `#[allow(dead_code)]` this carried ("Will be used when exec()
    /// implementation is complete") is stale on both architectures now — x86's
    /// production `exec_process`/`exec_process_with_argv` call this unconditionally,
    /// same as aarch64's already did.
    ///
    /// The blocked mask and the entire pending set survive exec, including a
    /// SIGKILL queued while the old image was reading from ext2.
    pub fn exec_reset(&mut self) {
        self.alt_stack = AltStack::default();
        for sig in 1..=NSIG {
            if self.get_handler(sig).is_handler() {
                self.set_handler(sig, SignalAction::default());
            }
            // SIG_IGN and SIG_DFL are preserved.
        }
    }
}

/// x86-64 `struct sigcontext`, the machine context of a Linux `ucontext_t`.
#[cfg(target_arch = "x86_64")]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SigContext {
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rax: u64,
    pub rcx: u64,
    pub rsp: u64,
    pub rip: u64,
    pub eflags: u64,
    pub cs: u16,
    pub gs: u16,
    pub fs: u16,
    pub ss: u16,
    pub err: u64,
    pub trapno: u64,
    pub oldmask: u64,
    pub cr2: u64,
    /// User address of the FXSAVE image saved with the frame, or 0.
    pub fpstate: u64,
    pub reserved: [u64; 8],
}

/// x86-64 Linux `ucontext_t` as the kernel writes it (uc_sigmask is the
/// kernel's 8-byte sigset).
#[cfg(target_arch = "x86_64")]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UContext {
    pub uc_flags: u64,
    pub uc_link: u64,
    pub uc_stack: StackT,
    pub uc_mcontext: SigContext,
    pub uc_sigmask: u64,
}

/// The x86-64 signal frame: Linux's `rt_sigframe`. The handler is entered
/// with RSP pointing at `pretcode`, as if called, so its `ret` goes to the
/// restorer, which calls rt_sigreturn with RSP just above `pretcode`. The
/// FXSAVE image `uc_mcontext.fpstate` points to lies above the frame.
#[cfg(target_arch = "x86_64")]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SignalFrame {
    pub pretcode: u64,
    pub uc: UContext,
    pub info: LinuxSigInfo,
}

#[cfg(target_arch = "x86_64")]
impl SignalFrame {
    /// Size of the signal frame in bytes
    pub const SIZE: usize = core::mem::size_of::<Self>();
}

/// ARM64 `__reserved` area of a `struct sigcontext`, in which the kernel
/// writes a sequence of records ending in an empty one.
#[cfg(target_arch = "aarch64")]
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy)]
pub struct SigContextReserved(pub [u8; 4096]);

/// ARM64 `struct sigcontext`.
#[cfg(target_arch = "aarch64")]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SigContext {
    pub fault_address: u64,
    pub regs: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub pstate: u64,
    /// The alignment gap before the 16-byte-aligned `__reserved`, a field so
    /// that every byte of a frame written to user memory is initialized.
    pub _pad: u64,
    pub reserved: SigContextReserved,
}

/// ARM64 Linux `ucontext_t`: uc_sigmask is padded to the 1024-bit sigset
/// glibc reserves, then the machine context.
#[cfg(target_arch = "aarch64")]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UContext {
    pub uc_flags: u64,
    pub uc_link: u64,
    pub uc_stack: StackT,
    pub uc_sigmask: u64,
    pub unused: [u8; 120],
    /// The alignment gap before `uc_mcontext`, a field for the same reason
    /// as `SigContext::_pad`.
    pub _pad: u64,
    pub uc_mcontext: SigContext,
}

/// The FP/SIMD record in an ARM64 sigcontext's `__reserved` area.
#[cfg(target_arch = "aarch64")]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FpsimdContext {
    pub magic: u32,
    pub size: u32,
    pub fpsr: u32,
    pub fpcr: u32,
    pub vregs: [u128; 32],
}

#[cfg(target_arch = "aarch64")]
impl FpsimdContext {
    pub const MAGIC: u32 = 0x4650_8001;
    pub const SIZE: u32 = core::mem::size_of::<Self>() as u32;
}

/// The ESR record in an ARM64 sigcontext's `__reserved` area, which a
/// fault signal's frame carries after the FP/SIMD record.
#[cfg(target_arch = "aarch64")]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EsrContext {
    pub magic: u32,
    pub size: u32,
    pub esr: u64,
}

#[cfg(target_arch = "aarch64")]
impl EsrContext {
    pub const MAGIC: u32 = 0x4553_5201;
    pub const SIZE: u32 = core::mem::size_of::<Self>() as u32;
}

/// The ARM64 signal frame: Linux's `rt_sigframe` followed by the frame
/// record (saved x29, x30) the handler's x29 points to. The handler is
/// entered with SP at `info` and x30 at the restorer, which calls
/// rt_sigreturn with SP unchanged.
#[cfg(target_arch = "aarch64")]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SignalFrame {
    pub info: LinuxSigInfo,
    pub uc: UContext,
    pub frame_record: [u64; 2],
}

#[cfg(target_arch = "aarch64")]
impl SignalFrame {
    /// Size of the signal frame in bytes
    pub const SIZE: usize = core::mem::size_of::<Self>();
}

// Each size is also the sum of its fields' sizes: no frame type has a gap
// that would carry uninitialized kernel bytes to user memory.
#[cfg(target_arch = "x86_64")]
const _: () = {
    assert!(core::mem::size_of::<SigContext>() == 256);
    assert!(core::mem::size_of::<SigContext>() == 18 * 8 + 4 * 2 + 5 * 8 + 8 * 8);
    assert!(core::mem::size_of::<StackT>() == 8 + 4 + 4 + 8);
    assert!(core::mem::size_of::<UContext>() == 8 + 8 + 24 + 256 + 8);
    assert!(core::mem::size_of::<SignalFrame>() == 8 + 304 + 128);
    assert!(core::mem::size_of::<UContext>() == 304);
    assert!(core::mem::size_of::<SignalFrame>() == 440);
};

#[cfg(target_arch = "aarch64")]
const _: () = {
    assert!(core::mem::size_of::<SigContext>() == 4384);
    assert!(core::mem::size_of::<SigContext>() == 8 + 31 * 8 + 3 * 8 + 8 + 4096);
    assert!(core::mem::offset_of!(UContext, uc_mcontext) == 176);
    assert!(core::mem::size_of::<UContext>() == 8 + 8 + 24 + 8 + 120 + 8 + 4384);
    assert!(core::mem::size_of::<SignalFrame>() == 128 + 4560 + 16);
    assert!(core::mem::size_of::<EsrContext>() == 16);
    assert!(core::mem::size_of::<UContext>() == 4560);
    assert!(core::mem::offset_of!(SignalFrame, uc) == 128);
    assert!(core::mem::size_of::<FpsimdContext>() == 528);
    assert!(core::mem::size_of::<SignalFrame>() == 4704);
};

// ============================================================================
// Interval Timer Types (for setitimer/getitimer)
// ============================================================================

/// Interval timer types (which parameter to setitimer/getitimer)
pub mod itimer {
    /// Real time timer - decrements in real time, delivers SIGALRM
    pub const ITIMER_REAL: i32 = 0;

    /// Virtual timer - decrements in process virtual time, delivers SIGVTALRM
    /// Only counts time when process is executing in user mode
    pub const ITIMER_VIRTUAL: i32 = 1;

    /// Profiling timer - decrements in process time, delivers SIGPROF
    /// Counts time when process is executing in user or kernel mode
    pub const ITIMER_PROF: i32 = 2;
}

/// Time value structure for interval timers (matches POSIX struct timeval)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Timeval {
    /// Seconds
    pub tv_sec: i64,
    /// Microseconds (must be < 1,000,000)
    pub tv_usec: i64,
}

impl Timeval {
    /// Create a zero timeval (represents "no time" or "timer disabled")
    pub const fn zero() -> Self {
        Timeval {
            tv_sec: 0,
            tv_usec: 0,
        }
    }

    /// Check if this timeval represents zero time
    pub fn is_zero(&self) -> bool {
        self.tv_sec == 0 && self.tv_usec == 0
    }

    /// Convert to microseconds
    pub fn to_micros(&self) -> u64 {
        if self.tv_sec < 0 || self.tv_usec < 0 {
            return 0;
        }
        (self.tv_sec as u64) * 1_000_000 + (self.tv_usec as u64)
    }

    /// Create from microseconds
    pub fn from_micros(micros: u64) -> Self {
        Timeval {
            tv_sec: (micros / 1_000_000) as i64,
            tv_usec: (micros % 1_000_000) as i64,
        }
    }
}

/// Interval timer value structure (matches POSIX struct itimerval)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Itimerval {
    /// Timer interval for periodic timers (zero = one-shot)
    pub it_interval: Timeval,
    /// Time until next expiration (zero = timer disabled)
    pub it_value: Timeval,
}

impl Itimerval {
    /// Create an empty/disabled timer
    pub const fn empty() -> Self {
        Itimerval {
            it_interval: Timeval::zero(),
            it_value: Timeval::zero(),
        }
    }

    /// Check if timer is disabled (it_value is zero)
    #[allow(dead_code)] // Part of Itimerval public API, called by user code
    pub fn is_disabled(&self) -> bool {
        self.it_value.is_zero()
    }
}

/// An absolute deadline in the timer's clock domain. Syscalls and scheduler
/// expiry serialize on this small lock; the scheduler only ever tries it.
#[derive(Default)]
pub struct IntervalTimer {
    value: spin::Mutex<(u64, u64)>,
    active: AtomicBool,
}

impl IntervalTimer {
    fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Release);
    }

    /// Disarm the timer, unless a syscall holds it.
    fn try_disarm(&self) {
        if let Some(mut value) = self.value.try_lock() {
            *value = (0, 0);
            self.set_active(false);
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Physical wall-clock deadline publication, outside the interrupt path.
    pub fn deadline_micros(&self) -> Option<u64> {
        if !self.is_active() { return None; }
        Some(self.value.try_lock().map_or(0, |value| value.0))
    }

    pub fn get_value(&self, now: u64) -> Itimerval {
        let value = self.value.lock();
        Itimerval {
            it_interval: Timeval::from_micros(value.1),
            it_value: Timeval::from_micros(if value.0 == 0 { 0 } else { value.0.saturating_sub(now).max(1) }),
        }
    }

    pub fn set_value(&self, new: &Itimerval, now: u64) -> Itimerval {
        let mut value = self.value.lock();
        let old = Itimerval {
            it_interval: Timeval::from_micros(value.1),
            it_value: Timeval::from_micros(if value.0 == 0 { 0 } else { value.0.saturating_sub(now).max(1) }),
        };
        let delay = new.it_value.to_micros();
        *value = (
            if delay == 0 {
                0
            } else {
                now.saturating_add(delay)
            },
            new.it_interval.to_micros(),
        );
        self.set_active(delay != 0);
        old
    }

    pub fn expire(&self, now: u64) -> bool {
        if !self.active.load(Ordering::Acquire) { return false; }
        let Some(mut value) = self.value.try_lock() else {
            return false;
        };
        if value.0 == 0 || now < value.0 {
            return false;
        }
        value.0 = if value.1 == 0 {
            0
        } else {
            // Preserve the periodic phase, coalescing missed expirations.
            now.saturating_add(value.1 - (now - value.0) % value.1)
        };
        self.set_active(value.0 != 0);
        true
    }
}

/// Clocks: monotonic microseconds, process user microseconds, process total
/// CPU microseconds. Thread-group rows share the same timers.
#[derive(Default)]
pub struct IntervalTimers {
    pub real: IntervalTimer,
    pub virtual_timer: IntervalTimer,
    pub prof: IntervalTimer,
    /// The process's POSIX timers, which the scheduler visits with these.
    pub posix: super::timers::PosixTimers,
}

impl IntervalTimers {
    /// Disarm every timer of a process whose threads have all exited. A timer
    /// a syscall holds is left for the scheduler's next pass.
    pub fn disarm_all(&self) {
        if !self.is_active() {
            return;
        }
        self.real.try_disarm();
        self.virtual_timer.try_disarm();
        self.prof.try_disarm();
        self.posix.try_disarm_all();
    }

    pub fn is_active(&self) -> bool {
        self.real.is_active() || self.virtual_timer.is_active() || self.prof.is_active() || self.posix.is_active()
    }

    pub fn timer(&self, which: i32) -> &IntervalTimer {
        match which {
            itimer::ITIMER_REAL => &self.real,
            itimer::ITIMER_VIRTUAL => &self.virtual_timer,
            _ => &self.prof,
        }
    }
}

pub fn monotonic_micros() -> u64 {
    let (sec, ns) = crate::time::get_monotonic_time_ns();
    sec.saturating_mul(1_000_000).saturating_add(ns / 1000)
}

/// Nanosecond clock for relative waits, without rounding their start down.
pub fn monotonic_nanos() -> u64 {
    let (sec, ns) = crate::time::get_monotonic_time_ns();
    sec.saturating_mul(1_000_000_000).saturating_add(ns)
}
