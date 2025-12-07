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

// sigaction flags
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
