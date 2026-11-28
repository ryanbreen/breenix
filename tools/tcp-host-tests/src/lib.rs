//! Host checks of the kernel's TCP sender. `net::tcp` is kernel/src/net/tcp.rs
//! with its `tcp/` submodules, compiled unchanged; the modules beside it here
//! stand in for the kernel services it calls. Segments enter through
//! `handle_tcp`, as the receive path delivers them, and leave through
//! `send_ipv4_from`, which `harness` records instead of transmitting. The clock
//! is `harness`'s fixed clock, and the timer worker's pass is the kernel's own
//! `run_timers`.
//!
//! Device output, the scheduler and the timer thread's sleep are outside these
//! checks; they settle the protocol's decisions, not its timing on a guest.
#![cfg(test)]
extern crate alloc;

#[path = "../../../kernel/src/syscall/errno.rs"]
pub mod errno;
pub mod syscall {
    pub use crate::errno;
}

pub mod harness;
pub mod net;
pub mod random;

pub mod time {
    /// The harness clock, as the kernel's monotonic clock reports it.
    pub fn get_monotonic_time_ns() -> (u64, u64) {
        let ms = crate::harness::now_ms();
        (ms / 1_000, (ms % 1_000) * 1_000_000)
    }
}

pub mod process {
    pub mod process {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct ProcessId(pub u64);
    }
}

/// No scheduler runs here: a block returns at once, and wakes find no thread.
pub mod task {
    pub mod scheduler {
        #[derive(Debug, PartialEq, Eq)]
        pub enum WakeOutcome {
            Applied,
            NoTarget,
        }
        pub struct Current;
        impl Current {
            pub fn block_current_for_timer(&mut self, _deadline_ns: u64) {}
            pub fn block_current(&mut self) {}
            pub fn block_current_for_io(&mut self) {}
        }
        pub fn with_current<R>(f: impl FnOnce(&mut Current) -> R) -> R {
            f(&mut Current)
        }
        pub fn current_thread_id() -> Option<u64> {
            Some(1)
        }
        pub fn yield_current() {}
        pub fn unblock_thread(_tid: u64) {}
        pub fn wake_thread_any_context(_tid: u64) -> WakeOutcome {
            WakeOutcome::NoTarget
        }
    }

    /// `ktcptimerd` does not start: the checks run its pass themselves.
    pub mod kthread {
        pub struct KthreadHandle;
        impl KthreadHandle {
            pub fn tid(&self) -> u64 {
                2
            }
        }
        pub fn kthread_run<F: FnOnce() + Send + 'static>(
            _func: F,
            _name: &str,
        ) -> Result<KthreadHandle, ()> {
            Ok(KthreadHandle)
        }
    }
}

pub mod ipc {
    pub mod poll {
        pub mod events {
            pub const POLLIN: i16 = 0x0001;
            pub const POLLPRI: i16 = 0x0002;
            pub const POLLOUT: i16 = 0x0004;
            pub const POLLERR: i16 = 0x0008;
            pub const POLLHUP: i16 = 0x0010;
            pub const POLLRDHUP: i16 = 0x2000;
        }
    }
}

/// The checks give connections no socket, so the notices reach no waiter.
pub mod socket {
    pub mod object {
        pub struct Socket {
            pub common: SocketCommon,
        }
        pub struct SocketCommon;
        impl SocketCommon {
            pub fn notify(&self) {}
            pub fn notify_readable(&self) {}
            pub fn notify_writable(&self) {}
            pub fn set_error(&self, _error: i32) {}
        }
    }
}

pub fn arch_without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    f()
}

pub fn arch_halt_with_interrupts() {}

mod newreno_checks;
mod offload_checks;
