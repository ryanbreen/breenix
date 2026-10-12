//! Keep catchable interruption signals from bypassing ChildGroup's Drop.

use std::sync::atomic::{AtomicBool, Ordering};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn mark_interrupted(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

pub struct SignalHandlers {
    previous: [(libc::c_int, libc::sighandler_t); 3],
}

impl SignalHandlers {
    pub fn install() -> Self {
        INTERRUPTED.store(false, Ordering::Relaxed);
        let previous = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP].map(|signal| {
            // SAFETY: the handler only writes a lock-free atomic, and is restored on drop.
            (signal, unsafe { libc::signal(signal, mark_interrupted as libc::sighandler_t) })
        });
        Self { previous }
    }

    pub fn interrupted() -> bool {
        INTERRUPTED.load(Ordering::Relaxed)
    }
}

impl Drop for SignalHandlers {
    fn drop(&mut self) {
        for (signal, previous) in self.previous {
            // SAFETY: restore the process's prior disposition after the owned child is reaped.
            unsafe { libc::signal(signal, previous) };
        }
    }
}
