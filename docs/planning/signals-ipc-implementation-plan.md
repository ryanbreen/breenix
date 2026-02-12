# Signals & IPC Implementation Plan

## Overview

This document describes the implementation plan for POSIX signals and basic IPC (pipes) in Breenix. The implementation follows the existing kernel patterns and respects the prohibited code sections.

## Current State Analysis

### Process Control Block (PCB)
Location: `kernel/src/process/process.rs`

The current `Process` struct has:
- Parent/child tracking (`parent: Option<ProcessId>`, `children: Vec<ProcessId>`)
- State management (`ProcessState` enum with Creating/Ready/Running/Blocked/Terminated)
- Thread association (`main_thread: Option<Thread>`)
- Memory management (page_table, heap, vmas)

**No signal-related fields exist** - this is greenfield work.

### Syscall Infrastructure
Location: `kernel/src/syscall/`

Pattern:
1. `mod.rs` - Defines `SyscallNumber` enum with `from_u64()` conversion
2. `dispatcher.rs` - Match on syscall number, dispatch to handlers
3. `handlers.rs` - Business logic for each syscall
4. Specialized modules (`time.rs`, `mmap.rs`, `memory.rs`) for complex syscalls

### Signal Delivery Point
Location: `kernel/src/interrupts/context_switch.rs`

The function `check_need_resched_and_switch()` is called on:
- Every timer interrupt return (when returning to userspace)
- Every syscall return

This is the **ideal location** to check for pending signals before returning to userspace.

### Userspace Library
Location: `libs/libbreenix/src/`

Pattern:
- `syscall.rs` - Raw syscall primitives (`syscall0` through `syscall6`)
- Module files (`process.rs`, `io.rs`, `time.rs`) - High-level wrappers
- `lib.rs` - Re-exports public APIs

---

## Implementation Phases

### Phase 1: Signal Infrastructure (Foundation)

**Goal**: Add signal state to processes and define signal constants.

#### 1.1 Create Signal Module
Create `kernel/src/signal/mod.rs`:

```rust
//! Signal handling infrastructure for Breenix

pub mod constants;
pub mod delivery;
pub mod types;

pub use constants::*;
pub use types::*;
```

#### 1.2 Signal Constants
Create `kernel/src/signal/constants.rs`:

```rust
//! Signal numbers following Linux x86_64 conventions

// Standard signals (1-31)
pub const SIGHUP: u32 = 1;
pub const SIGINT: u32 = 2;
pub const SIGQUIT: u32 = 3;
pub const SIGILL: u32 = 4;
pub const SIGTRAP: u32 = 5;
pub const SIGABRT: u32 = 6;
pub const SIGBUS: u32 = 7;
pub const SIGFPE: u32 = 8;
pub const SIGKILL: u32 = 9;    // Cannot be caught or blocked
pub const SIGUSR1: u32 = 10;
pub const SIGSEGV: u32 = 11;
pub const SIGUSR2: u32 = 12;
pub const SIGPIPE: u32 = 13;
pub const SIGALRM: u32 = 14;
pub const SIGTERM: u32 = 15;
pub const SIGSTKFLT: u32 = 16;
pub const SIGCHLD: u32 = 17;
pub const SIGCONT: u32 = 18;
pub const SIGSTOP: u32 = 19;   // Cannot be caught or blocked
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

// Real-time signals (32-64) - future work
pub const SIGRTMIN: u32 = 32;
pub const SIGRTMAX: u32 = 64;

// Signal handler special values
pub const SIG_DFL: u64 = 0;  // Default action
pub const SIG_IGN: u64 = 1;  // Ignore signal

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

/// Maximum signal number
pub const NSIG: u32 = 64;

/// Convert signal number to bit mask
#[inline]
pub fn sig_mask(sig: u32) -> u64 {
    if sig == 0 || sig > NSIG {
        0
    } else {
        1u64 << (sig - 1)
    }
}

/// Signals that cannot be caught or blocked
pub const UNCATCHABLE_SIGNALS: u64 = sig_mask(SIGKILL) | sig_mask(SIGSTOP);
```

#### 1.3 Signal Types
Create `kernel/src/signal/types.rs`:

```rust
//! Signal-related data structures

use super::constants::*;

/// Default action for a signal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalDefaultAction {
    Terminate,
    Ignore,
    CoreDump,
    Stop,
    Continue,
}

/// Get the default action for a signal
pub fn default_action(sig: u32) -> SignalDefaultAction {
    match sig {
        SIGHUP | SIGINT | SIGKILL | SIGPIPE | SIGALRM | SIGTERM |
        SIGUSR1 | SIGUSR2 | SIGIO | SIGPWR | SIGSTKFLT => SignalDefaultAction::Terminate,

        SIGQUIT | SIGILL | SIGTRAP | SIGABRT | SIGBUS | SIGFPE |
        SIGSEGV | SIGXCPU | SIGXFSZ | SIGSYS => SignalDefaultAction::CoreDump,

        SIGCHLD | SIGURG | SIGWINCH => SignalDefaultAction::Ignore,

        SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU => SignalDefaultAction::Stop,

        SIGCONT => SignalDefaultAction::Continue,

        _ => SignalDefaultAction::Terminate,
    }
}

/// Signal handler configuration
#[derive(Debug, Clone, Copy)]
pub struct SignalAction {
    /// Handler address (SIG_DFL, SIG_IGN, or user function)
    pub handler: u64,
    /// Signals to block during handler execution
    pub mask: u64,
    /// Flags (SA_RESTART, SA_SIGINFO, etc.)
    pub flags: u64,
    /// Restorer function for sigreturn (optional, kernel provides trampoline)
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

/// Per-process signal state
#[derive(Clone)]
pub struct SignalState {
    /// Pending signals bitmap (signals waiting to be delivered)
    pub pending: u64,
    /// Blocked signals bitmap (sigprocmask)
    pub blocked: u64,
    /// Signal handlers (one per signal, 1-64)
    pub handlers: [SignalAction; 64],
}

impl Default for SignalState {
    fn default() -> Self {
        SignalState {
            pending: 0,
            blocked: 0,
            handlers: [SignalAction::default(); 64],
        }
    }
}

impl SignalState {
    /// Check if any signals are pending and not blocked
    #[inline]
    pub fn has_deliverable_signals(&self) -> bool {
        (self.pending & !self.blocked) != 0
    }

    /// Get the next deliverable signal (lowest number first)
    pub fn next_deliverable_signal(&self) -> Option<u32> {
        let deliverable = self.pending & !self.blocked;
        if deliverable == 0 {
            return None;
        }
        // Find lowest set bit (trailing zeros)
        let bit = deliverable.trailing_zeros();
        Some(bit + 1)  // Signal numbers are 1-based
    }

    /// Mark a signal as pending
    pub fn set_pending(&mut self, sig: u32) {
        self.pending |= sig_mask(sig);
    }

    /// Clear a pending signal
    pub fn clear_pending(&mut self, sig: u32) {
        self.pending &= !sig_mask(sig);
    }

    /// Check if a signal is blocked
    pub fn is_blocked(&self, sig: u32) -> bool {
        (self.blocked & sig_mask(sig)) != 0
    }

    /// Get handler for a signal
    pub fn get_handler(&self, sig: u32) -> &SignalAction {
        &self.handlers[(sig - 1) as usize]
    }

    /// Set handler for a signal
    pub fn set_handler(&mut self, sig: u32, action: SignalAction) {
        self.handlers[(sig - 1) as usize] = action;
    }
}
```

#### 1.4 Add Signal State to Process
Modify `kernel/src/process/process.rs`:

```rust
// Add to imports
use crate::signal::SignalState;

// Add to Process struct
pub struct Process {
    // ... existing fields ...

    /// Signal handling state
    pub signals: SignalState,
}

// Update Process::new()
impl Process {
    pub fn new(id: ProcessId, name: String, entry_point: VirtAddr) -> Self {
        Process {
            // ... existing field initializations ...
            signals: SignalState::default(),
        }
    }
}
```

#### 1.5 Add Signal Syscall Numbers
Modify `kernel/src/syscall/mod.rs`:

```rust
pub enum SyscallNumber {
    // ... existing syscalls ...

    // Signal syscalls (Linux x86_64 numbers)
    Sigaction = 13,     // rt_sigaction
    Sigprocmask = 14,   // rt_sigprocmask
    Sigreturn = 15,     // rt_sigreturn
    Kill = 62,          // kill
    Sigpending = 127,   // rt_sigpending
    Sigsuspend = 130,   // rt_sigsuspend
    Sigaltstack = 131,  // sigaltstack
}

// Update from_u64()
impl SyscallNumber {
    pub fn from_u64(value: u64) -> Option<Self> {
        match value {
            // ... existing cases ...
            13 => Some(Self::Sigaction),
            14 => Some(Self::Sigprocmask),
            15 => Some(Self::Sigreturn),
            62 => Some(Self::Kill),
            127 => Some(Self::Sigpending),
            130 => Some(Self::Sigsuspend),
            131 => Some(Self::Sigaltstack),
            _ => None,
        }
    }
}
```

#### 1.6 Update Kernel lib.rs
Add module to `kernel/src/lib.rs`:

```rust
pub mod signal;
```

---

### Phase 2: Basic kill Syscall

**Goal**: Implement the ability to send signals between processes.

#### 2.1 Create Signal Syscall Module
Create `kernel/src/syscall/signal.rs`:

```rust
//! Signal-related system calls

use crate::signal::{constants::*, types::*};
use crate::process::{ProcessId, manager};
use super::SyscallResult;

/// kill(pid, sig) - Send signal to a process
///
/// pid > 0: Send to process with that PID
/// pid == 0: Send to all processes in caller's process group (not implemented)
/// pid == -1: Send to all processes (not implemented)
/// pid < -1: Send to process group (not implemented)
pub fn sys_kill(pid: i64, sig: i32) -> SyscallResult {
    let sig = sig as u32;

    // Validate signal number
    if sig == 0 {
        // Signal 0 is used to check if process exists
        return check_process_exists(pid);
    }

    if sig > NSIG {
        return SyscallResult::Err(22); // EINVAL
    }

    if pid > 0 {
        // Send to specific process
        send_signal_to_process(ProcessId::new(pid as u64), sig)
    } else if pid == 0 {
        // Send to process group (not implemented)
        log::warn!("kill(0, {}) - process groups not implemented", sig);
        SyscallResult::Err(38) // ENOSYS
    } else if pid == -1 {
        // Send to all processes (not implemented)
        log::warn!("kill(-1, {}) - broadcast not implemented", sig);
        SyscallResult::Err(38) // ENOSYS
    } else {
        // Send to process group -pid (not implemented)
        log::warn!("kill({}, {}) - process groups not implemented", pid, sig);
        SyscallResult::Err(38) // ENOSYS
    }
}

fn check_process_exists(pid: i64) -> SyscallResult {
    if pid <= 0 {
        return SyscallResult::Err(22); // EINVAL
    }

    let target_pid = ProcessId::new(pid as u64);
    let manager_guard = manager();

    if let Some(ref manager) = *manager_guard {
        if manager.get_process(target_pid).is_some() {
            return SyscallResult::Ok(0);
        }
    }

    SyscallResult::Err(3) // ESRCH - No such process
}

fn send_signal_to_process(target_pid: ProcessId, sig: u32) -> SyscallResult {
    let mut manager_guard = manager();

    if let Some(ref mut manager) = *manager_guard {
        if let Some(process) = manager.get_process_mut(target_pid) {
            // Check if process is alive
            if process.is_terminated() {
                return SyscallResult::Err(3); // ESRCH
            }

            // SIGKILL and SIGSTOP cannot be caught or blocked
            if sig == SIGKILL {
                // Terminate immediately
                log::info!("SIGKILL sent to process {}", target_pid.as_u64());
                process.terminate(-9); // Killed by signal
                // Wake up process if blocked
                if matches!(process.state, crate::process::ProcessState::Blocked) {
                    process.set_ready();
                }
                return SyscallResult::Ok(0);
            }

            if sig == SIGSTOP {
                // Stop process
                log::info!("SIGSTOP sent to process {}", target_pid.as_u64());
                process.set_blocked(); // Use Blocked state for stopped
                return SyscallResult::Ok(0);
            }

            // For other signals, set pending bit
            process.signals.set_pending(sig);

            // Wake up process if blocked (so it can handle the signal)
            if matches!(process.state, crate::process::ProcessState::Blocked) {
                process.set_ready();
            }

            log::debug!("Signal {} queued for process {}", sig, target_pid.as_u64());
            SyscallResult::Ok(0)
        } else {
            SyscallResult::Err(3) // ESRCH - No such process
        }
    } else {
        SyscallResult::Err(3) // ESRCH
    }
}
```

#### 2.2 Update Dispatcher
Add to `kernel/src/syscall/dispatcher.rs`:

```rust
SyscallNumber::Kill => super::signal::sys_kill(arg1 as i64, arg2 as i32),
```

#### 2.3 Update Userspace Library
Create `libs/libbreenix/src/signal.rs`:

```rust
//! Signal handling for userspace programs

use crate::syscall::raw;

// Syscall numbers
pub const SYS_KILL: u64 = 62;
pub const SYS_SIGACTION: u64 = 13;
pub const SYS_SIGPROCMASK: u64 = 14;
pub const SYS_SIGRETURN: u64 = 15;

// Re-export signal constants
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
pub const SIGCHLD: i32 = 17;
pub const SIGCONT: i32 = 18;
pub const SIGSTOP: i32 = 19;

/// Send signal to a process
pub fn kill(pid: i32, sig: i32) -> Result<(), i32> {
    let ret = unsafe { raw::syscall2(SYS_KILL, pid as u64, sig as u64) };
    if ret == 0 {
        Ok(())
    } else {
        Err(ret as i32)
    }
}
```

Update `libs/libbreenix/src/lib.rs`:
```rust
pub mod signal;
pub use signal::kill;
```

---

### Phase 3: Signal Delivery Mechanism

**Goal**: Deliver pending signals when returning to userspace.

#### 3.1 Signal Delivery Logic
Create `kernel/src/signal/delivery.rs`:

```rust
//! Signal delivery to userspace

use super::{constants::*, types::*};
use crate::process::{Process, ProcessId};
use crate::task::thread::Thread;

/// Check and deliver pending signals
///
/// Called from check_need_resched_and_switch() before returning to userspace.
/// Returns true if a signal was delivered and interrupt frame was modified.
pub fn check_and_deliver_signal(
    process: &mut Process,
    thread: &mut Thread,
    interrupt_frame: &mut x86_64::structures::idt::InterruptStackFrame,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
) -> bool {
    // Get next deliverable signal
    let sig = match process.signals.next_deliverable_signal() {
        Some(s) => s,
        None => return false,
    };

    // Clear pending flag
    process.signals.clear_pending(sig);

    let action = process.signals.get_handler(sig);

    match action.handler {
        SIG_DFL => deliver_default_action(process, sig),
        SIG_IGN => {
            log::debug!("Signal {} ignored by process {}", sig, process.id.as_u64());
            false
        }
        handler_addr => {
            // User-defined handler - set up signal frame
            deliver_to_user_handler(process, thread, interrupt_frame, saved_regs, sig, handler_addr, action)
        }
    }
}

fn deliver_default_action(process: &mut Process, sig: u32) -> bool {
    match default_action(sig) {
        SignalDefaultAction::Terminate => {
            log::info!("Process {} terminated by signal {}", process.id.as_u64(), sig);
            process.terminate(-(sig as i32)); // Exit code is -signal
            true
        }
        SignalDefaultAction::CoreDump => {
            log::info!("Process {} core dumped by signal {}", process.id.as_u64(), sig);
            // TODO: Actual core dump support
            process.terminate(-(sig as i32) | 0x80); // Core dump flag
            true
        }
        SignalDefaultAction::Stop => {
            log::info!("Process {} stopped by signal {}", process.id.as_u64(), sig);
            process.set_blocked();
            true
        }
        SignalDefaultAction::Continue => {
            log::info!("Process {} continued by signal {}", process.id.as_u64(), sig);
            if matches!(process.state, crate::process::ProcessState::Blocked) {
                process.set_ready();
            }
            false
        }
        SignalDefaultAction::Ignore => {
            log::debug!("Signal {} ignored (default) by process {}", sig, process.id.as_u64());
            false
        }
    }
}

fn deliver_to_user_handler(
    process: &mut Process,
    thread: &mut Thread,
    interrupt_frame: &mut x86_64::structures::idt::InterruptStackFrame,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
    sig: u32,
    handler_addr: u64,
    action: &SignalAction,
) -> bool {
    // TODO: Implement in Phase 5
    // For now, just log and ignore
    log::warn!(
        "User signal handler at {:#x} for signal {} not yet implemented",
        handler_addr, sig
    );
    false
}
```

#### 3.2 Integrate into Context Switch Path
Modify `kernel/src/interrupts/context_switch.rs`:

In `restore_userspace_thread_context()`, before returning, add signal check:

```rust
// SIGNAL DELIVERY POINT
// Check for pending signals before returning to userspace
if process.signals.has_deliverable_signals() {
    if crate::signal::delivery::check_and_deliver_signal(
        process,
        thread,
        interrupt_frame,
        saved_regs,
    ) {
        // Signal was delivered - frame may have been modified
        // Process may have been terminated
        if process.is_terminated() {
            // Schedule away from terminated process
            crate::task::scheduler::set_need_resched();
        }
    }
}
```

**NOTE**: This is a Tier 2 modification to `context_switch.rs`. The signal check is a simple bitmap check (O(1)) and will not add significant overhead.

---

### Phase 4: sigaction and sigprocmask Syscalls

**Goal**: Allow processes to install custom signal handlers and block signals.

#### 4.1 sigaction Implementation
Add to `kernel/src/syscall/signal.rs`:

```rust
/// sigaction structure (Linux x86_64 ABI)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SigactionUser {
    pub handler: u64,
    pub flags: u64,
    pub restorer: u64,
    pub mask: u64,
}

/// rt_sigaction(sig, act, oldact, sigsetsize)
pub fn sys_sigaction(
    sig: i32,
    new_act: u64,  // Pointer to SigactionUser
    old_act: u64,  // Pointer to SigactionUser
    sigsetsize: u64,
) -> SyscallResult {
    let sig = sig as u32;

    // Validate signal number
    if sig == 0 || sig > NSIG {
        return SyscallResult::Err(22); // EINVAL
    }

    // Cannot change handler for SIGKILL or SIGSTOP
    if sig == SIGKILL || sig == SIGSTOP {
        return SyscallResult::Err(22); // EINVAL
    }

    // sigsetsize must be 8 (size of u64 bitmask)
    if sigsetsize != 8 {
        return SyscallResult::Err(22); // EINVAL
    }

    // Get current process
    let current_thread_id = crate::task::scheduler::current_thread_id()
        .ok_or_else(|| SyscallResult::Err(3))?; // ESRCH

    let mut manager_guard = manager();
    let manager = manager_guard.as_mut().ok_or_else(|| SyscallResult::Err(3))?;
    let (_, process) = manager.find_process_by_thread_mut(current_thread_id)
        .ok_or_else(|| SyscallResult::Err(3))?;

    // Save old action if requested
    if old_act != 0 {
        let old_action = process.signals.get_handler(sig);
        let user_old = SigactionUser {
            handler: old_action.handler,
            flags: old_action.flags,
            restorer: old_action.restorer,
            mask: old_action.mask,
        };
        unsafe {
            core::ptr::write(old_act as *mut SigactionUser, user_old);
        }
    }

    // Set new action if provided
    if new_act != 0 {
        let user_new = unsafe { core::ptr::read(new_act as *const SigactionUser) };
        let new_action = SignalAction {
            handler: user_new.handler,
            flags: user_new.flags,
            restorer: user_new.restorer,
            mask: user_new.mask & !UNCATCHABLE_SIGNALS, // Cannot block SIGKILL/SIGSTOP
        };
        process.signals.set_handler(sig, new_action);
        log::debug!("Set signal {} handler to {:#x}", sig, new_action.handler);
    }

    SyscallResult::Ok(0)
}

/// rt_sigprocmask(how, set, oldset, sigsetsize)
pub fn sys_sigprocmask(
    how: i32,
    new_set: u64,  // Pointer to u64 bitmask
    old_set: u64,  // Pointer to u64 bitmask
    sigsetsize: u64,
) -> SyscallResult {
    // sigsetsize must be 8
    if sigsetsize != 8 {
        return SyscallResult::Err(22); // EINVAL
    }

    // Get current process
    let current_thread_id = crate::task::scheduler::current_thread_id()
        .ok_or_else(|| SyscallResult::Err(3))?;

    let mut manager_guard = manager();
    let manager = manager_guard.as_mut().ok_or_else(|| SyscallResult::Err(3))?;
    let (_, process) = manager.find_process_by_thread_mut(current_thread_id)
        .ok_or_else(|| SyscallResult::Err(3))?;

    // Save old mask if requested
    if old_set != 0 {
        unsafe {
            core::ptr::write(old_set as *mut u64, process.signals.blocked);
        }
    }

    // Modify mask if new_set is provided
    if new_set != 0 {
        let set = unsafe { core::ptr::read(new_set as *const u64) };
        // Cannot block SIGKILL or SIGSTOP
        let set = set & !UNCATCHABLE_SIGNALS;

        match how {
            SIG_BLOCK => {
                process.signals.blocked |= set;
            }
            SIG_UNBLOCK => {
                process.signals.blocked &= !set;
            }
            SIG_SETMASK => {
                process.signals.blocked = set;
            }
            _ => return SyscallResult::Err(22), // EINVAL
        }
    }

    SyscallResult::Ok(0)
}
```

#### 4.2 Update Dispatcher
```rust
SyscallNumber::Sigaction => super::signal::sys_sigaction(
    arg1 as i32, arg2, arg3, arg4
),
SyscallNumber::Sigprocmask => super::signal::sys_sigprocmask(
    arg1 as i32, arg2, arg3, arg4
),
```

#### 4.3 Update Userspace Library
Add to `libs/libbreenix/src/signal.rs`:

```rust
/// Signal action structure
#[repr(C)]
pub struct Sigaction {
    pub handler: u64,
    pub flags: u64,
    pub restorer: u64,
    pub mask: u64,
}

/// Set signal handler
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

    if ret == 0 {
        Ok(())
    } else {
        Err(ret as i32)
    }
}

/// Block/unblock signals
pub fn sigprocmask(how: i32, set: Option<&u64>, oldset: Option<&mut u64>) -> Result<(), i32> {
    let set_ptr = set.map_or(0, |s| s as *const _ as u64);
    let oldset_ptr = oldset.map_or(0, |s| s as *mut _ as u64);

    let ret = unsafe {
        raw::syscall4(SYS_SIGPROCMASK, how as u64, set_ptr, oldset_ptr, 8)
    };

    if ret == 0 {
        Ok(())
    } else {
        Err(ret as i32)
    }
}
```

---

### Phase 5: sigreturn and User Handlers

**Goal**: Enable user-defined signal handlers with proper context save/restore.

This is the most complex phase, requiring:

1. **Signal frame structure** on user stack
2. **Signal trampoline** for sigreturn
3. **Context save/restore** for signal delivery

#### 5.1 Signal Frame Structure

```rust
/// Signal frame pushed to user stack before calling handler
#[repr(C)]
pub struct SignalFrame {
    /// Return address to signal trampoline
    pub trampoline_addr: u64,
    /// Signal number (RDI argument)
    pub signal: u64,
    /// Signal info pointer (RSI argument) - future
    pub siginfo_ptr: u64,
    /// Context pointer (RDX argument) - future
    pub ucontext_ptr: u64,
    /// Saved registers
    pub saved_rip: u64,
    pub saved_rsp: u64,
    pub saved_rflags: u64,
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
    /// Blocked signals before handler
    pub saved_blocked: u64,
}
```

#### 5.2 Signal Delivery to User Handler

Update `deliver_to_user_handler()` in `kernel/src/signal/delivery.rs`:

```rust
fn deliver_to_user_handler(
    process: &mut Process,
    thread: &mut Thread,
    interrupt_frame: &mut x86_64::structures::idt::InterruptStackFrame,
    saved_regs: &mut SavedRegisters,
    sig: u32,
    handler_addr: u64,
    action: &SignalAction,
) -> bool {
    // Get current user RSP
    let user_rsp = unsafe { interrupt_frame.as_ref().stack_pointer.as_u64() };

    // Allocate space for signal frame on user stack
    let frame_size = core::mem::size_of::<SignalFrame>() as u64;
    let new_rsp = (user_rsp - frame_size) & !0xF; // 16-byte aligned

    // Build signal frame
    let frame = SignalFrame {
        trampoline_addr: process.signal_trampoline, // Set during process creation
        signal: sig as u64,
        siginfo_ptr: 0,
        ucontext_ptr: 0,
        saved_rip: unsafe { interrupt_frame.as_ref().instruction_pointer.as_u64() },
        saved_rsp: user_rsp,
        saved_rflags: unsafe { interrupt_frame.as_ref().cpu_flags.bits() },
        saved_rax: saved_regs.rax,
        saved_rbx: saved_regs.rbx,
        saved_rcx: saved_regs.rcx,
        saved_rdx: saved_regs.rdx,
        saved_rdi: saved_regs.rdi,
        saved_rsi: saved_regs.rsi,
        saved_rbp: saved_regs.rbp,
        saved_r8: saved_regs.r8,
        saved_r9: saved_regs.r9,
        saved_r10: saved_regs.r10,
        saved_r11: saved_regs.r11,
        saved_r12: saved_regs.r12,
        saved_r13: saved_regs.r13,
        saved_r14: saved_regs.r14,
        saved_r15: saved_regs.r15,
        saved_blocked: process.signals.blocked,
    };

    // Write frame to user stack
    unsafe {
        core::ptr::write(new_rsp as *mut SignalFrame, frame);
    }

    // Block signals during handler (SA_NODEFER allows recursive signals)
    if (action.flags & SA_NODEFER) == 0 {
        process.signals.blocked |= sig_mask(sig);
    }
    process.signals.blocked |= action.mask;

    // Set up interrupt frame to jump to handler
    unsafe {
        interrupt_frame.as_mut().update(|f| {
            f.instruction_pointer = x86_64::VirtAddr::new(handler_addr);
            f.stack_pointer = x86_64::VirtAddr::new(new_rsp);
        });
    }

    // Set up arguments: RDI = signal number
    saved_regs.rdi = sig as u64;
    saved_regs.rsi = 0; // siginfo_t* (future)
    saved_regs.rdx = 0; // ucontext_t* (future)

    log::debug!(
        "Delivering signal {} to handler {:#x}, new RSP={:#x}",
        sig, handler_addr, new_rsp
    );

    true
}
```

#### 5.3 sigreturn Implementation

```rust
/// rt_sigreturn() - Return from signal handler
///
/// Restores the pre-signal context from the signal frame on user stack
pub fn sys_sigreturn(frame: &mut crate::syscall::handler::SyscallFrame) -> SyscallResult {
    // Get signal frame from user stack
    // The signal frame starts at RSP (trampoline pushed return address already consumed)
    let frame_ptr = frame.rsp as *const SignalFrame;
    let signal_frame = unsafe { core::ptr::read(frame_ptr) };

    // Get current process
    let current_thread_id = crate::task::scheduler::current_thread_id()
        .ok_or_else(|| SyscallResult::Err(3))?;

    let mut manager_guard = manager();
    let manager = manager_guard.as_mut().ok_or_else(|| SyscallResult::Err(3))?;
    let (_, process) = manager.find_process_by_thread_mut(current_thread_id)
        .ok_or_else(|| SyscallResult::Err(3))?;

    // Restore blocked signals
    process.signals.blocked = signal_frame.saved_blocked & !UNCATCHABLE_SIGNALS;

    // Restore registers to syscall frame (will be restored on return)
    frame.rax = signal_frame.saved_rax;
    frame.rbx = signal_frame.saved_rbx;
    frame.rcx = signal_frame.saved_rcx;
    frame.rdx = signal_frame.saved_rdx;
    frame.rdi = signal_frame.saved_rdi;
    frame.rsi = signal_frame.saved_rsi;
    frame.rbp = signal_frame.saved_rbp;
    frame.r8 = signal_frame.saved_r8;
    frame.r9 = signal_frame.saved_r9;
    frame.r10 = signal_frame.saved_r10;
    frame.r11 = signal_frame.saved_r11;
    frame.r12 = signal_frame.saved_r12;
    frame.r13 = signal_frame.saved_r13;
    frame.r14 = signal_frame.saved_r14;
    frame.r15 = signal_frame.saved_r15;

    // Restore instruction pointer and stack pointer
    frame.rip = signal_frame.saved_rip;
    frame.rsp = signal_frame.saved_rsp;
    frame.rflags = signal_frame.saved_rflags;

    log::debug!("sigreturn: restored RIP={:#x}, RSP={:#x}", frame.rip, frame.rsp);

    // Return value is whatever was in RAX before the signal
    SyscallResult::Ok(signal_frame.saved_rax)
}
```

#### 5.4 Signal Trampoline

The signal trampoline is a small piece of code mapped into every process that calls sigreturn after the handler returns:

```asm
; Signal trampoline - called when signal handler returns
signal_trampoline:
    mov rax, 15         ; SYS_sigreturn
    int 0x80            ; syscall
    ; Should never return, but if it does:
    mov rax, 0          ; SYS_exit
    mov rdi, 255        ; exit code
    int 0x80
```

This can be implemented as a special page mapped into process address space during creation.

---

### Phase 6: Pipes (IPC)

**Goal**: Implement anonymous pipes for inter-process communication.

#### 6.1 File Descriptor Infrastructure

First, we need basic file descriptor support. Add to `kernel/src/process/process.rs`:

```rust
/// Maximum number of open file descriptors
pub const FD_MAX: usize = 256;

/// File descriptor table
pub struct FdTable {
    fds: [Option<FileDescriptor>; FD_MAX],
    next_fd: usize,
}

/// A file descriptor
#[derive(Clone)]
pub struct FileDescriptor {
    pub kind: FdKind,
    pub flags: u32,
}

/// Kind of file descriptor
#[derive(Clone)]
pub enum FdKind {
    /// Standard I/O (stdin=0, stdout=1, stderr=2)
    StdIo(u32),
    /// Pipe read end
    PipeRead(alloc::sync::Arc<PipeBuffer>),
    /// Pipe write end
    PipeWrite(alloc::sync::Arc<PipeBuffer>),
}
```

#### 6.2 Pipe Buffer

Create `kernel/src/ipc/mod.rs`:

```rust
pub mod pipe;
```

Create `kernel/src/ipc/pipe.rs`:

```rust
//! Pipe implementation

use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::spinlock::SpinLock;

/// Pipe buffer size (64KB like Linux)
pub const PIPE_BUF_SIZE: usize = 65536;

/// Pipe buffer
pub struct PipeBuffer {
    data: SpinLock<PipeInner>,
}

struct PipeInner {
    buffer: Vec<u8>,
    read_pos: usize,
    write_pos: usize,
    readers: u32,
    writers: u32,
}

impl PipeBuffer {
    pub fn new() -> Arc<Self> {
        Arc::new(PipeBuffer {
            data: SpinLock::new(PipeInner {
                buffer: alloc::vec![0u8; PIPE_BUF_SIZE],
                read_pos: 0,
                write_pos: 0,
                readers: 1,
                writers: 1,
            }),
        })
    }

    /// Read from pipe, returns bytes read or error
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, i32> {
        let mut inner = self.data.lock();

        // Calculate available data
        let available = if inner.write_pos >= inner.read_pos {
            inner.write_pos - inner.read_pos
        } else {
            PIPE_BUF_SIZE - inner.read_pos + inner.write_pos
        };

        if available == 0 {
            if inner.writers == 0 {
                // No writers, EOF
                return Ok(0);
            }
            // Would block
            return Err(11); // EAGAIN
        }

        // Read data
        let to_read = buf.len().min(available);
        let mut read = 0;

        while read < to_read {
            buf[read] = inner.buffer[inner.read_pos];
            inner.read_pos = (inner.read_pos + 1) % PIPE_BUF_SIZE;
            read += 1;
        }

        Ok(read)
    }

    /// Write to pipe, returns bytes written or error
    pub fn write(&self, buf: &[u8]) -> Result<usize, i32> {
        let mut inner = self.data.lock();

        if inner.readers == 0 {
            // No readers, SIGPIPE
            return Err(32); // EPIPE
        }

        // Calculate free space
        let used = if inner.write_pos >= inner.read_pos {
            inner.write_pos - inner.read_pos
        } else {
            PIPE_BUF_SIZE - inner.read_pos + inner.write_pos
        };
        let free = PIPE_BUF_SIZE - 1 - used; // -1 to distinguish full from empty

        if free == 0 {
            // Would block
            return Err(11); // EAGAIN
        }

        // Write data
        let to_write = buf.len().min(free);
        let mut written = 0;

        while written < to_write {
            inner.buffer[inner.write_pos] = buf[written];
            inner.write_pos = (inner.write_pos + 1) % PIPE_BUF_SIZE;
            written += 1;
        }

        Ok(written)
    }

    pub fn close_read(&self) {
        let mut inner = self.data.lock();
        inner.readers = inner.readers.saturating_sub(1);
    }

    pub fn close_write(&self) {
        let mut inner = self.data.lock();
        inner.writers = inner.writers.saturating_sub(1);
    }
}
```

#### 6.3 pipe Syscall

Create `kernel/src/syscall/pipe.rs`:

```rust
//! Pipe system call

use crate::ipc::pipe::PipeBuffer;
use crate::process::FdKind;
use super::SyscallResult;

/// pipe(pipefd[2]) - Create a pipe
pub fn sys_pipe(pipefd: u64) -> SyscallResult {
    // Get current process
    let current_thread_id = crate::task::scheduler::current_thread_id()
        .ok_or_else(|| SyscallResult::Err(3))?;

    let mut manager_guard = crate::process::manager();
    let manager = manager_guard.as_mut().ok_or_else(|| SyscallResult::Err(3))?;
    let (_, process) = manager.find_process_by_thread_mut(current_thread_id)
        .ok_or_else(|| SyscallResult::Err(3))?;

    // Create pipe buffer
    let buffer = PipeBuffer::new();

    // Allocate file descriptors
    let read_fd = process.fd_table.allocate(FdKind::PipeRead(buffer.clone()))
        .ok_or_else(|| SyscallResult::Err(24))?; // EMFILE
    let write_fd = process.fd_table.allocate(FdKind::PipeWrite(buffer))
        .ok_or_else(|| {
            process.fd_table.close(read_fd);
            SyscallResult::Err(24)
        })?;

    // Write fds to user pointer
    unsafe {
        let fds = pipefd as *mut [i32; 2];
        (*fds)[0] = read_fd as i32;
        (*fds)[1] = write_fd as i32;
    }

    log::debug!("Created pipe: read_fd={}, write_fd={}", read_fd, write_fd);
    SyscallResult::Ok(0)
}
```

#### 6.4 Add Syscall Numbers

```rust
// In SyscallNumber enum
Pipe = 22,
Pipe2 = 293,
Close = 3,  // For closing pipe ends
```

---

## Testing Strategy

### Boot Stage Markers

Add to `xtask/src/main.rs`:

```rust
BootStage {
    name: "Signal kill test passed",
    marker: "SIGNAL_KILL_TEST_PASSED",
    failure_meaning: "kill() syscall or signal delivery broken",
    check_hint: "kernel/src/syscall/signal.rs - sys_kill()",
},
BootStage {
    name: "Signal handler test passed",
    marker: "SIGNAL_HANDLER_TEST_PASSED",
    failure_meaning: "sigaction() or signal delivery to user handler broken",
    check_hint: "kernel/src/signal/delivery.rs - deliver_to_user_handler()",
},
BootStage {
    name: "Pipe test passed",
    marker: "PIPE_TEST_PASSED",
    failure_meaning: "pipe() syscall or pipe I/O broken",
    check_hint: "kernel/src/syscall/pipe.rs - sys_pipe()",
},
```

### Test Programs

Create test programs in `userspace/programs/`:

#### Test 1: Basic kill (SIGTERM)
```rust
// test_signal_kill.rs
#[no_mangle]
pub extern "C" fn _start() -> ! {
    let pid = fork();
    if pid == 0 {
        // Child: loop forever
        loop { yield_now(); }
    }

    // Parent: wait a bit, then kill child
    for _ in 0..100 { yield_now(); }

    if kill(pid, SIGTERM) == Ok(()) {
        println("SIGNAL_KILL_TEST_PASSED");
    }
    exit(0);
}
```

#### Test 2: Custom Signal Handler
```rust
// test_signal_handler.rs
static mut HANDLER_CALLED: bool = false;

extern "C" fn handler(_sig: i32) {
    unsafe { HANDLER_CALLED = true; }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let action = Sigaction {
        handler: handler as u64,
        flags: 0,
        restorer: 0,
        mask: 0,
    };

    sigaction(SIGUSR1, Some(&action), None).unwrap();
    kill(getpid(), SIGUSR1).unwrap();

    if unsafe { HANDLER_CALLED } {
        println("SIGNAL_HANDLER_TEST_PASSED");
    }
    exit(0);
}
```

#### Test 3: Pipe Communication
```rust
// test_pipe.rs
#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut fds = [0i32; 2];
    pipe(&mut fds).unwrap();

    let pid = fork();
    if pid == 0 {
        // Child: write to pipe
        close(fds[0]);
        write(fds[1], b"hello");
        exit(0);
    }

    // Parent: read from pipe
    close(fds[1]);
    let mut buf = [0u8; 5];
    read(fds[0], &mut buf);

    if &buf == b"hello" {
        println("PIPE_TEST_PASSED");
    }
    exit(0);
}
```

---

## Implementation Order Summary

1. **Phase 1**: Signal infrastructure (types, constants, PCB fields)
2. **Phase 2**: `kill` syscall (basic signal sending)
3. **Phase 3**: Signal delivery in context switch path
4. **Phase 4**: `sigaction` and `sigprocmask` syscalls
5. **Phase 5**: `sigreturn` and user signal handlers
6. **Phase 6**: Pipes (`pipe` syscall, pipe buffer)

Each phase builds on the previous and can be tested independently.

---

## Critical Constraints

### Prohibited Files
- `kernel/src/syscall/handler.rs` - Only add dispatch cases, no logic
- `kernel/src/syscall/time.rs` - Do not modify
- `kernel/src/interrupts/timer.rs` - Do not modify

### Tier 2 Files (High Scrutiny)
- `kernel/src/interrupts/context_switch.rs` - Signal check must be fast (bitmap only)

### Performance Requirements
- Signal check: O(1) bitmap operation
- No logging in hot paths
- No allocations during signal delivery check
