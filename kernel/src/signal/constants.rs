//! Signal numbers and constants following Linux x86_64 conventions

// Standard signals (1-31)
pub const SIGHUP: u32 = 1;
pub const SIGINT: u32 = 2;
pub const SIGQUIT: u32 = 3;
pub const SIGILL: u32 = 4;
pub const SIGTRAP: u32 = 5;
pub const SIGABRT: u32 = 6;
pub const SIGBUS: u32 = 7;
pub const SIGFPE: u32 = 8;
pub const SIGKILL: u32 = 9; // Cannot be caught or blocked
pub const SIGUSR1: u32 = 10;
pub const SIGSEGV: u32 = 11;
pub const SIGUSR2: u32 = 12;
pub const SIGPIPE: u32 = 13;
pub const SIGALRM: u32 = 14;
pub const SIGTERM: u32 = 15;
pub const SIGSTKFLT: u32 = 16;
pub const SIGCHLD: u32 = 17;
pub const SIGCONT: u32 = 18;
pub const SIGSTOP: u32 = 19; // Cannot be caught or blocked
pub const SIGTSTP: u32 = 20;
pub const SIGTTIN: u32 = 21;
pub const SIGTTOU: u32 = 22;
/// The stop signals whose default action stops the process.
pub const STOP_SIGNALS: u64 =
    (1 << (SIGSTOP - 1)) | (1 << (SIGTSTP - 1)) | (1 << (SIGTTIN - 1)) | (1 << (SIGTTOU - 1));
pub const SIGURG: u32 = 23;
pub const SIGXCPU: u32 = 24;
pub const SIGXFSZ: u32 = 25;
pub const SIGVTALRM: u32 = 26;
pub const SIGPROF: u32 = 27;
pub const SIGWINCH: u32 = 28;
pub const SIGIO: u32 = 29;
pub const SIGPWR: u32 = 30;
pub const SIGSYS: u32 = 31;

// Real-time signals (32-64) - for future use
pub const SIGRTMIN: u32 = 32;
pub const SIGRTMAX: u32 = 64;

/// Maximum signal number supported
pub const NSIG: u32 = 64;

// Signal handler special values
/// Default action for the signal
pub const SIG_DFL: u64 = 0;
/// Ignore the signal
pub const SIG_IGN: u64 = 1;

// sigprocmask "how" values
/// Block signals in set
pub const SIG_BLOCK: i32 = 0;
/// Unblock signals in set
pub const SIG_UNBLOCK: i32 = 1;
/// Set blocked signals to set
pub const SIG_SETMASK: i32 = 2;

// sigaltstack flags
/// Currently executing on alternate signal stack
pub const SS_ONSTACK: u32 = 1;
/// Alternate signal stack is disabled
pub const SS_DISABLE: u32 = 2;
/// Linux: disarm the alternate stack while a handler runs on it
pub const SS_AUTODISARM: u32 = 1 << 31;
/// Minimum size for an alternate signal stack: the Linux ABI's value, which
/// holds one signal frame on this architecture.
#[cfg(target_arch = "x86_64")]
pub const MINSIGSTKSZ: usize = 2048;
#[cfg(target_arch = "aarch64")]
pub const MINSIGSTKSZ: usize = 5120;

// sigaction flags
/// SIGCHLD only: no SIGCHLD when a child stops or continues
pub const SA_NOCLDSTOP: u64 = 0x00000001;
/// SIGCHLD only: exited children do not become zombies
pub const SA_NOCLDWAIT: u64 = 0x00000002;
/// Restart interrupted syscalls
#[allow(dead_code)] // Part of POSIX sigaction API, used by userspace
pub const SA_RESTART: u64 = 0x10000000;
/// Don't block signal during handler
pub const SA_NODEFER: u64 = 0x40000000;
/// Provide siginfo_t to handler
#[allow(dead_code)] // Part of POSIX sigaction API, used by userspace
pub const SA_SIGINFO: u64 = 0x00000004;
/// Use alternate signal stack
#[allow(dead_code)] // Part of POSIX sigaction API, used by userspace
pub const SA_ONSTACK: u64 = 0x08000000;
/// Provide restorer function
#[allow(dead_code)] // Part of POSIX sigaction API, used by userspace
pub const SA_RESTORER: u64 = 0x04000000;
/// Reset the action to SIG_DFL on entry to the handler
pub const SA_RESETHAND: u64 = 0x80000000;

// siginfo si_code values (Linux ABI)
/// Sent by kill or raise
pub const SI_USER: i32 = 0;
/// Sent by the kernel
pub const SI_KERNEL: i32 = 0x80;
/// Sent by sigqueue
pub const SI_QUEUE: i32 = -1;
/// Sent by tkill or tgkill
pub const SI_TKILL: i32 = -6;
/// SIGSEGV: address not mapped
pub const SEGV_MAPERR: i32 = 1;
/// SIGSEGV: access not permitted by the mapping
pub const SEGV_ACCERR: i32 = 2;
/// SIGBUS: misaligned address
pub const BUS_ADRALN: i32 = 1;
/// SIGBUS: no backing for the address
pub const BUS_ADRERR: i32 = 2;
/// SIGBUS: a hardware error at the address (external abort, parity or ECC)
pub const BUS_OBJERR: i32 = 3;
/// SIGTRAP: a breakpoint instruction
pub const TRAP_BRKPT: i32 = 1;
/// SIGTRAP: a single step
pub const TRAP_TRACE: i32 = 2;
/// SIGTRAP: a hardware breakpoint or watchpoint
pub const TRAP_HWBKPT: i32 = 4;
/// SIGILL: illegal opcode
pub const ILL_ILLOPC: i32 = 1;
/// SIGILL: illegal operand
pub const ILL_ILLOPN: i32 = 2;
/// SIGFPE: integer divide by zero
pub const FPE_INTDIV: i32 = 1;
/// SIGFPE: floating-point divide by zero
pub const FPE_FLTDIV: i32 = 3;
/// SIGFPE: floating-point overflow
pub const FPE_FLTOVF: i32 = 4;
/// SIGFPE: floating-point underflow
pub const FPE_FLTUND: i32 = 5;
/// SIGFPE: floating-point inexact result
pub const FPE_FLTRES: i32 = 6;
/// SIGFPE: invalid floating-point operation
pub const FPE_FLTINV: i32 = 7;
/// SIGCHLD: the child exited
pub const CLD_EXITED: i32 = 1;
/// SIGCHLD: the child was killed
pub const CLD_KILLED: i32 = 2;
/// SIGCHLD: the child was killed and dumped core
pub const CLD_DUMPED: i32 = 3;
/// SIGCHLD: the child stopped
pub const CLD_STOPPED: i32 = 5;
/// SIGCHLD: the stopped child continued
pub const CLD_CONTINUED: i32 = 6;

/// Convert signal number to bit mask
///
/// Returns 0 for invalid signal numbers (0 or > NSIG)
#[inline]
pub const fn sig_mask(sig: u32) -> u64 {
    if sig == 0 || sig > NSIG {
        0
    } else {
        1u64 << (sig - 1)
    }
}

/// Signals that cannot be caught, blocked, or ignored
pub const UNCATCHABLE_SIGNALS: u64 = sig_mask(SIGKILL) | sig_mask(SIGSTOP);

/// Signals a fault raises, which are taken before any other pending signal
/// so that the handler sees the fault's context (as Linux's next_signal).
pub const SYNCHRONOUS_SIGNALS: u64 = sig_mask(SIGSEGV)
    | sig_mask(SIGBUS)
    | sig_mask(SIGILL)
    | sig_mask(SIGTRAP)
    | sig_mask(SIGFPE)
    | sig_mask(SIGSYS);

/// Check if a signal number is valid
#[inline]
pub const fn is_valid_signal(sig: u32) -> bool {
    sig > 0 && sig <= NSIG
}

/// Check if a signal can be caught/blocked
#[inline]
pub const fn is_catchable(sig: u32) -> bool {
    sig != SIGKILL && sig != SIGSTOP
}

/// Get signal name for debugging
pub fn signal_name(sig: u32) -> &'static str {
    match sig {
        SIGHUP => "SIGHUP",
        SIGINT => "SIGINT",
        SIGQUIT => "SIGQUIT",
        SIGILL => "SIGILL",
        SIGTRAP => "SIGTRAP",
        SIGABRT => "SIGABRT",
        SIGBUS => "SIGBUS",
        SIGFPE => "SIGFPE",
        SIGKILL => "SIGKILL",
        SIGUSR1 => "SIGUSR1",
        SIGSEGV => "SIGSEGV",
        SIGUSR2 => "SIGUSR2",
        SIGPIPE => "SIGPIPE",
        SIGALRM => "SIGALRM",
        SIGTERM => "SIGTERM",
        SIGSTKFLT => "SIGSTKFLT",
        SIGCHLD => "SIGCHLD",
        SIGCONT => "SIGCONT",
        SIGSTOP => "SIGSTOP",
        SIGTSTP => "SIGTSTP",
        SIGTTIN => "SIGTTIN",
        SIGTTOU => "SIGTTOU",
        SIGURG => "SIGURG",
        SIGXCPU => "SIGXCPU",
        SIGXFSZ => "SIGXFSZ",
        SIGVTALRM => "SIGVTALRM",
        SIGPROF => "SIGPROF",
        SIGWINCH => "SIGWINCH",
        SIGIO => "SIGIO",
        SIGPWR => "SIGPWR",
        SIGSYS => "SIGSYS",
        _ if sig >= SIGRTMIN && sig <= SIGRTMAX => "SIGRT",
        _ => "UNKNOWN",
    }
}
