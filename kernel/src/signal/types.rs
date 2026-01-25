//! Signal-related data structures

use super::constants::*;

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

/// Per-process alternate signal stack state
///
/// Stores the configured alternate stack and whether we're currently
/// executing on it.
#[derive(Debug, Clone, Copy, Default)]
pub struct AltStack {
    /// Base address of the alternate stack
    pub base: u64,
    /// Size of the alternate stack in bytes
    pub size: usize,
    /// Flags (SS_DISABLE if disabled)
    pub flags: u32,
    /// True if currently executing a signal handler on this stack
    pub on_stack: bool,
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

/// Signal handler configuration (matches Linux sigaction structure layout)
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct SignalAction {
    /// Handler address (SIG_DFL, SIG_IGN, or user function pointer)
    pub handler: u64,
    /// Signals to block during handler execution
    pub mask: u64,
    /// Flags (SA_RESTART, SA_SIGINFO, etc.)
    pub flags: u64,
    /// Restorer function for sigreturn (provided by libc or kernel)
    pub restorer: u64,
}

impl Default for SignalAction {
    fn default() -> Self {
        SignalAction {
            handler: SIG_DFL,
            mask: 0,
            flags: 0,
            restorer: 0,
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
    #[allow(dead_code)] // Part of complete signal API, will be used for signal dispatch optimization
    pub fn is_ignore(&self) -> bool {
        self.handler == SIG_IGN
    }

    /// Check if handler is a user function
    #[inline]
    #[allow(dead_code)] // Part of complete signal API, will be used for signal dispatch
    pub fn is_user_handler(&self) -> bool {
        self.handler > SIG_IGN
    }
}

/// Per-process signal state
///
/// Note: handlers are boxed to avoid stack overflow. The 64-element array
/// is 2KB (64 * 32 bytes) which causes stack overflow during process creation
/// if stored inline.
#[derive(Clone)]
pub struct SignalState {
    /// Pending signals bitmap (signals waiting to be delivered)
    pub pending: u64,
    /// Blocked signals bitmap (sigprocmask)
    pub blocked: u64,
    /// Signal handlers (one per signal, indices 0-63 for signals 1-64)
    /// Boxed to avoid stack overflow - 64 * 32 bytes = 2KB
    handlers: alloc::boxed::Box<[SignalAction; 64]>,
    /// Alternate signal stack configuration
    pub alt_stack: AltStack,
    /// Saved signal mask from sigsuspend - restored after signal handler returns via sigreturn
    /// This is set when sigsuspend temporarily changes the mask and a signal is delivered.
    /// The sigreturn syscall checks this and restores the original mask.
    pub sigsuspend_saved_mask: Option<u64>,
}

impl Default for SignalState {
    fn default() -> Self {
        SignalState {
            pending: 0,
            blocked: 0,
            handlers: alloc::boxed::Box::new([SignalAction::default(); 64]),
            alt_stack: AltStack::default(),
            sigsuspend_saved_mask: None,
        }
    }
}

impl SignalState {
    /// Create a new signal state with default handlers
    #[allow(dead_code)] // Used by Default trait, part of public API
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if any signals are pending and not blocked
    #[inline]
    pub fn has_deliverable_signals(&self) -> bool {
        (self.pending & !self.blocked) != 0
    }

    /// Get the next deliverable signal (lowest number first)
    ///
    /// Returns None if no signals are pending and unblocked
    pub fn next_deliverable_signal(&self) -> Option<u32> {
        let deliverable = self.pending & !self.blocked;
        if deliverable == 0 {
            return None;
        }
        // Find lowest set bit (trailing zeros gives the bit position)
        let bit = deliverable.trailing_zeros();
        Some(bit + 1) // Signal numbers are 1-based
    }

    /// Mark a signal as pending
    #[inline]
    pub fn set_pending(&mut self, sig: u32) {
        if is_valid_signal(sig) {
            self.pending |= sig_mask(sig);
        }
    }

    /// Clear a pending signal
    #[inline]
    pub fn clear_pending(&mut self, sig: u32) {
        if is_valid_signal(sig) {
            self.pending &= !sig_mask(sig);
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
        (self.blocked & sig_mask(sig)) != 0
    }

    /// Get handler for a signal
    ///
    /// Returns the default handler for invalid signal numbers
    pub fn get_handler(&self, sig: u32) -> &SignalAction {
        if sig == 0 || sig > NSIG {
            // Return a static default for invalid signals
            static DEFAULT: SignalAction = SignalAction {
                handler: SIG_DFL,
                mask: 0,
                flags: 0,
                restorer: 0,
            };
            &DEFAULT
        } else {
            &self.handlers[(sig - 1) as usize]
        }
    }

    /// Set handler for a signal
    ///
    /// Does nothing for invalid signal numbers
    pub fn set_handler(&mut self, sig: u32, action: SignalAction) {
        if sig > 0 && sig <= NSIG {
            self.handlers[(sig - 1) as usize] = action;
        }
    }

    /// Block additional signals
    #[inline]
    pub fn block_signals(&mut self, mask: u64) {
        // Cannot block SIGKILL or SIGSTOP
        self.blocked |= mask & !UNCATCHABLE_SIGNALS;
    }

    /// Unblock signals
    #[inline]
    pub fn unblock_signals(&mut self, mask: u64) {
        self.blocked &= !mask;
    }

    /// Set the blocked signal mask
    #[inline]
    pub fn set_blocked(&mut self, mask: u64) {
        // Cannot block SIGKILL or SIGSTOP
        self.blocked = mask & !UNCATCHABLE_SIGNALS;
    }

    /// Fork the signal state for a child process
    ///
    /// Pending signals are NOT inherited, but handlers, mask, and alt stack are
    #[allow(dead_code)] // Will be used when fork() implementation is complete
    pub fn fork(&self) -> Self {
        SignalState {
            pending: 0, // Child starts with no pending signals
            blocked: self.blocked,
            handlers: self.handlers.clone(),
            alt_stack: self.alt_stack, // Alt stack is inherited per POSIX
            sigsuspend_saved_mask: None, // Child doesn't inherit sigsuspend state
        }
    }

    /// Reset signal handlers to default after exec
    ///
    /// Per POSIX, caught signals are reset to SIG_DFL, ignored signals stay ignored
    #[allow(dead_code)] // Will be used when exec() implementation is complete
    pub fn exec_reset(&mut self) {
        self.pending = 0;
        for handler in self.handlers.iter_mut() {
            if handler.is_user_handler() {
                *handler = SignalAction::default();
            }
            // SIG_IGN and SIG_DFL are preserved
        }
    }
}

/// Signal frame structure pushed to user stack when delivering a signal
///
/// This structure contains all state needed to restore execution after
/// the signal handler returns via sigreturn().
///
/// CRITICAL: trampoline_addr MUST be at offset 0!
/// When the signal handler executes 'ret', it pops from RSP.
/// RSP points to the start of SignalFrame, so trampoline_addr must be first.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SignalFrame {
    // Return address points to signal trampoline that calls sigreturn
    // MUST BE AT OFFSET 0 - this is what 'ret' will pop!
    pub trampoline_addr: u64,

    // Magic number for integrity checking (prevents privilege escalation)
    pub magic: u64,

    // Arguments for signal handler (in registers, but saved here too)
    pub signal: u64,     // Signal number (also in RDI)
    pub siginfo_ptr: u64, // Pointer to siginfo_t (also in RSI) - future
    pub ucontext_ptr: u64, // Pointer to ucontext_t (also in RDX) - future

    // Saved CPU state to restore after handler
    pub saved_rip: u64,
    pub saved_rsp: u64,
    pub saved_rflags: u64,

    // Saved general-purpose registers
    pub saved_rax: u64,
    pub saved_rbx: u64,
    pub saved_rcx: u64,
    pub saved_rdx: u64,
    pub saved_rdi: u64,
    pub saved_rsi: u64,
    pub saved_rbp: u64,
    pub saved_r8: u64,
    pub saved_r9: u64,
    pub saved_r10: u64,
    pub saved_r11: u64,
    pub saved_r12: u64,
    pub saved_r13: u64,
    pub saved_r14: u64,
    pub saved_r15: u64,

    // Signal state to restore
    pub saved_blocked: u64,
}

impl SignalFrame {
    /// Size of the signal frame in bytes
    pub const SIZE: usize = core::mem::size_of::<Self>();

    /// Magic number for frame integrity validation
    /// This prevents privilege escalation via forged signal frames
    pub const MAGIC: u64 = 0xDEAD_BEEF_CAFE_BABE;
}

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
    pub fn is_disabled(&self) -> bool {
        self.it_value.is_zero()
    }
}

/// Per-process interval timer state
///
/// This tracks a single interval timer with remaining time and repeat interval.
/// The kernel decrements remaining time on timer ticks and fires the appropriate
/// signal when it expires. If interval is non-zero, the timer automatically rearms.
#[derive(Debug, Clone)]
pub struct IntervalTimer {
    /// Time remaining until expiration in microseconds
    /// Zero means timer is disabled
    remaining_usec: u64,
    /// Repeat interval in microseconds
    /// Zero means one-shot (timer stops after firing once)
    interval_usec: u64,
}

impl Default for IntervalTimer {
    fn default() -> Self {
        IntervalTimer {
            remaining_usec: 0,
            interval_usec: 0,
        }
    }
}

impl IntervalTimer {
    /// Create a new disabled timer
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if timer is active (has remaining time)
    pub fn is_active(&self) -> bool {
        self.remaining_usec > 0
    }

    /// Get the current timer value as Itimerval
    pub fn get_value(&self) -> Itimerval {
        Itimerval {
            it_interval: Timeval::from_micros(self.interval_usec),
            it_value: Timeval::from_micros(self.remaining_usec),
        }
    }

    /// Set the timer from an Itimerval
    ///
    /// Returns the old value before setting
    pub fn set_value(&mut self, new_value: &Itimerval) -> Itimerval {
        let old = self.get_value();

        self.interval_usec = new_value.it_interval.to_micros();
        self.remaining_usec = new_value.it_value.to_micros();

        old
    }

    /// Decrement the timer by elapsed microseconds
    ///
    /// Returns true if the timer expired (and should fire its signal).
    /// If the timer has an interval, it automatically rearms.
    pub fn tick(&mut self, elapsed_usec: u64) -> bool {
        if self.remaining_usec == 0 {
            return false;
        }

        if elapsed_usec >= self.remaining_usec {
            // Timer expired
            if self.interval_usec > 0 {
                // Periodic timer - rearm with interval
                // Account for any overrun by subtracting the elapsed time
                // from the interval (but don't go negative)
                let overrun = elapsed_usec - self.remaining_usec;
                if overrun >= self.interval_usec {
                    // Multiple intervals elapsed - just reset to interval
                    self.remaining_usec = self.interval_usec;
                } else {
                    self.remaining_usec = self.interval_usec - overrun;
                }
            } else {
                // One-shot timer - disable
                self.remaining_usec = 0;
            }
            true
        } else {
            // Timer still running
            self.remaining_usec -= elapsed_usec;
            false
        }
    }
}

/// Collection of per-process interval timers
#[derive(Debug, Clone, Default)]
pub struct IntervalTimers {
    /// ITIMER_REAL - counts real (wall clock) time, fires SIGALRM
    pub real: IntervalTimer,
    /// ITIMER_VIRTUAL - counts user CPU time, fires SIGVTALRM
    #[allow(dead_code)] pub virtual_timer: IntervalTimer,
    /// ITIMER_PROF - counts user + system CPU time, fires SIGPROF
    #[allow(dead_code)] pub prof: IntervalTimer,
}
