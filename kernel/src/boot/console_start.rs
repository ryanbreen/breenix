//! Schedule console init after input preparation, retaining serial login on failure.
use crate::task;

pub fn schedule(thread: alloc::boxed::Box<task::thread::Thread>) {
    // Boot holds a scheduling brake here. Wait on an ordinary thread so
    // the xHCI worker can finish before init can start getty.
    // Keep ownership outside the closure until its creation succeeds: a
    // failed kthread_run drops the closure, but init still needs to run.
    let pending = alloc::sync::Arc::new(spin::Mutex::new(Some(thread)));
    let startup = pending.clone();
    if task::kthread::kthread_run(
        move || {
            let thread = startup.lock().take().expect("console init thread missing");
            if crate::drivers::xhci::wait_console_ready() {
                task::scheduler::spawn(thread);
            } else {
                log::error!(
                    "[console] keyboard preparation failed; starting init for serial login"
                );
                task::scheduler::spawn(thread);
            }
        },
        "console-start",
    )
    .is_err()
    {
        let thread = pending.lock().take().expect("console init thread missing");
        log::error!("[console] cannot create startup thread; starting init for serial login");
        task::scheduler::spawn(thread);
    }
}
