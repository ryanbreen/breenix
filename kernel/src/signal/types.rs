//! Signal-related data structures

use super::constants::*;

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
}

impl Default for SignalState {
    fn default() -> Self {
        SignalState {
            pending: 0,
            blocked: 0,
            handlers: alloc::boxed::Box::new([SignalAction::default(); 64]),
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
    /// Pending signals are NOT inherited, but handlers and mask are
    #[allow(dead_code)] // Will be used when fork() implementation is complete
    pub fn fork(&self) -> Self {
        SignalState {
            pending: 0, // Child starts with no pending signals
            blocked: self.blocked,
            handlers: self.handlers.clone(),
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
