//! Signal handling for userspace programs
//!
//! This module provides userspace wrappers for signal-related syscalls.

use crate::syscall::raw;

// Syscall numbers (must match kernel/src/syscall/mod.rs)
pub const SYS_SIGACTION: u64 = 13;
pub const SYS_SIGPROCMASK: u64 = 14;
pub const SYS_SIGRETURN: u64 = 15;
pub const SYS_KILL: u64 = 62;

// Signal numbers (must match kernel/src/signal/constants.rs)
pub const SIGHUP: i32 = 1;
pub const SIGINT: i32 = 2;
pub const SIGQUIT: i32 = 3;
pub const SIGILL: i32 = 4;
pub const SIGTRAP: i32 = 5;
pub const SIGABRT: i32 = 6;
pub const SIGBUS: i32 = 7;
pub const SIGFPE: i32 = 8;
pub const SIGKILL: i32 = 9;
pub const SIGUSR1: i32 = 10;
pub const SIGSEGV: i32 = 11;
pub const SIGUSR2: i32 = 12;
pub const SIGPIPE: i32 = 13;
pub const SIGALRM: i32 = 14;
pub const SIGTERM: i32 = 15;
pub const SIGSTKFLT: i32 = 16;
pub const SIGCHLD: i32 = 17;
pub const SIGCONT: i32 = 18;
pub const SIGSTOP: i32 = 19;
pub const SIGTSTP: i32 = 20;
pub const SIGTTIN: i32 = 21;
pub const SIGTTOU: i32 = 22;
pub const SIGURG: i32 = 23;
pub const SIGXCPU: i32 = 24;
pub const SIGXFSZ: i32 = 25;
pub const SIGVTALRM: i32 = 26;
pub const SIGPROF: i32 = 27;
pub const SIGWINCH: i32 = 28;
pub const SIGIO: i32 = 29;
pub const SIGPWR: i32 = 30;
pub const SIGSYS: i32 = 31;

// Signal handler special values
pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;

// sigprocmask "how" values
pub const SIG_BLOCK: i32 = 0;
pub const SIG_UNBLOCK: i32 = 1;
pub const SIG_SETMASK: i32 = 2;

// sigaction flags
pub const SA_RESTART: u64 = 0x10000000;
pub const SA_NODEFER: u64 = 0x40000000;
pub const SA_SIGINFO: u64 = 0x00000004;
pub const SA_ONSTACK: u64 = 0x08000000;
pub const SA_RESTORER: u64 = 0x04000000;

/// Signal action structure (must match kernel layout)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Sigaction {
    /// Handler function pointer, SIG_DFL, or SIG_IGN
    pub handler: u64,
    /// Signals to block during handler execution
    pub mask: u64,
    /// Flags (SA_RESTART, SA_SIGINFO, etc.)
    pub flags: u64,
    /// Restorer function (for sigreturn)
    pub restorer: u64,
}

impl Default for Sigaction {
    fn default() -> Self {
        Sigaction {
            handler: SIG_DFL,
            mask: 0,
            flags: 0,
            restorer: 0,
        }
    }
}

impl Sigaction {
    /// Create a new signal action with a handler function
    pub fn new(handler: extern "C" fn(i32)) -> Self {
        Sigaction {
            handler: handler as u64,
            mask: 0,
            flags: 0,
            restorer: 0,
        }
    }

    /// Create a signal action that ignores the signal
    pub fn ignore() -> Self {
        Sigaction {
            handler: SIG_IGN,
            mask: 0,
            flags: 0,
            restorer: 0,
        }
    }

    /// Create a signal action with default behavior
    pub fn default_action() -> Self {
        Sigaction::default()
    }
}

/// Send a signal to a process
///
/// # Arguments
/// * `pid` - Process ID to send signal to
/// * `sig` - Signal number to send
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(errno)` on failure
///
/// # Example
/// ```ignore
/// // Send SIGTERM to process 42
/// kill(42, SIGTERM)?;
///
/// // Check if process exists (sig=0)
/// if kill(42, 0).is_ok() {
///     // Process exists
/// }
/// ```
pub fn kill(pid: i32, sig: i32) -> Result<(), i32> {
    let ret = unsafe { raw::syscall2(SYS_KILL, pid as u64, sig as u64) };
    // Return value is 0 on success, negative errno on failure
    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok(())
    }
}

/// Set signal handler
///
/// # Arguments
/// * `sig` - Signal number to set handler for
/// * `act` - New signal action, or None to query current
/// * `oldact` - Where to store old action, or None
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(errno)` on failure
///
/// # Example
/// ```ignore
/// extern "C" fn handler(sig: i32) {
///     // Handle signal
/// }
///
/// let action = Sigaction::new(handler);
/// sigaction(SIGUSR1, Some(&action), None)?;
/// ```
pub fn sigaction(
    sig: i32,
    act: Option<&Sigaction>,
    oldact: Option<&mut Sigaction>,
) -> Result<(), i32> {
    let act_ptr = act.map_or(0, |a| a as *const _ as u64);
    let oldact_ptr = oldact.map_or(0, |a| a as *mut _ as u64);

    let ret = unsafe {
        raw::syscall4(SYS_SIGACTION, sig as u64, act_ptr, oldact_ptr, 8)
    };

    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok(())
    }
}

/// Block, unblock, or set the signal mask
///
/// # Arguments
/// * `how` - SIG_BLOCK, SIG_UNBLOCK, or SIG_SETMASK
/// * `set` - Signal mask to apply
/// * `oldset` - Where to store old mask, or None
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(errno)` on failure
///
/// # Example
/// ```ignore
/// // Block SIGINT
/// let mask = 1u64 << (SIGINT - 1);
/// sigprocmask(SIG_BLOCK, Some(&mask), None)?;
///
/// // Unblock all signals
/// let empty = 0u64;
/// sigprocmask(SIG_SETMASK, Some(&empty), None)?;
/// ```
pub fn sigprocmask(how: i32, set: Option<&u64>, oldset: Option<&mut u64>) -> Result<(), i32> {
    let set_ptr = set.map_or(0, |s| s as *const _ as u64);
    let oldset_ptr = oldset.map_or(0, |s| s as *mut _ as u64);

    let ret = unsafe {
        raw::syscall4(SYS_SIGPROCMASK, how as u64, set_ptr, oldset_ptr, 8)
    };

    if (ret as i64) < 0 {
        Err(-(ret as i64) as i32)
    } else {
        Ok(())
    }
}

/// Return from signal handler
///
/// This should be called at the end of a signal handler to restore
/// the pre-signal execution context. Usually called via a trampoline
/// rather than directly.
///
/// # Safety
/// This function never returns normally. It restores execution to
/// the point where the signal was delivered.
pub unsafe fn sigreturn() -> ! {
    raw::syscall0(SYS_SIGRETURN);
    // Should never reach here, but if it does, loop forever
    loop {
        core::hint::spin_loop();
    }
}

/// Convert signal number to bitmask
#[inline]
pub const fn sigmask(sig: i32) -> u64 {
    if sig <= 0 || sig > 64 {
        0
    } else {
        1u64 << (sig - 1)
    }
}

/// Get signal name for debugging
pub fn signame(sig: i32) -> &'static str {
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
        _ => "UNKNOWN",
    }
}
