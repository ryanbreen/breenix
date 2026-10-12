//! Keep catchable interruption signals from bypassing ChildGroup's Drop.

use std::sync::atomic::{AtomicI32, Ordering};

static INTERRUPTED: AtomicI32 = AtomicI32::new(0);

extern "C" fn mark_interrupted(signal: libc::c_int) {
    let _ = INTERRUPTED.compare_exchange(0, signal, Ordering::Relaxed, Ordering::Relaxed);
}

pub struct SignalHandlers {
    previous: [(libc::c_int, libc::sighandler_t); 3],
}

impl SignalHandlers {
    pub fn install() -> Self {
        assert!(
            !Self::interrupted(),
            "refusing a new runner after interruption"
        );
        let previous = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP].map(|signal| {
            // SAFETY: the handler only writes a lock-free atomic, and is restored on drop.
            (signal, unsafe {
                libc::signal(signal, mark_interrupted as libc::sighandler_t)
            })
        });
        Self { previous }
    }

    pub fn interrupted() -> bool {
        INTERRUPTED.load(Ordering::Relaxed) != 0
    }
}

impl Drop for SignalHandlers {
    fn drop(&mut self) {
        let interrupted = INTERRUPTED.load(Ordering::Relaxed);
        for (signal, previous) in self.previous {
            // SAFETY: restore the process's prior disposition after the owned child is reaped.
            unsafe { libc::signal(signal, previous) };
        }
        if interrupted != 0 {
            // Cleanup has finished; preserve the shell-visible signal status.
            unsafe {
                libc::signal(interrupted, libc::SIG_DFL);
                libc::raise(interrupted);
            }
        }
    }
}
