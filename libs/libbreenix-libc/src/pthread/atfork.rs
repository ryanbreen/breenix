//! pthread_atfork (POSIX.1-2024): handlers fork() runs around the process
//! copy, prepare handlers in reverse order of registration, parent and child
//! handlers in order of registration.
use super::*;

type Handler = Option<unsafe extern "C" fn()>;
/// This library's table size. POSIX lets registration fail with ENOMEM when
/// the table holding the handler addresses is full.
const MAX: usize = 64;
static mut TABLE: [[Handler; 3]; MAX] = [[None; 3]; MAX];
/// Entries below COUNT are complete; registration publishes with Release.
static COUNT: AtomicUsize = AtomicUsize::new(0);
static TABLE_LOCK: Lock = Lock::new();

#[no_mangle]
pub unsafe extern "C" fn pthread_atfork(prepare: Handler, parent: Handler, child: Handler) -> i32 {
    TABLE_LOCK.lock();
    let n = COUNT.load(Relaxed);
    if n == MAX {
        TABLE_LOCK.unlock();
        return ENOMEM;
    }
    (*core::ptr::addr_of_mut!(TABLE))[n] = [prepare, parent, child];
    COUNT.store(n + 1, Release);
    TABLE_LOCK.unlock();
    0
}
unsafe fn run(kind: usize, reverse: bool) {
    let n = COUNT.load(Acquire);
    for i in 0..n {
        let i = if reverse { n - 1 - i } else { i };
        if let Some(f) = (*core::ptr::addr_of!(TABLE))[i][kind] {
            f();
        }
    }
}
pub(crate) unsafe fn atfork_prepare() {
    run(0, true);
}
pub(crate) unsafe fn atfork_parent() {
    run(1, false);
}
/// In the child: after the runtime is repaired (`__breenix_after_fork`),
/// where only the forking thread exists and no registration is in progress.
pub(crate) unsafe fn atfork_child() {
    TABLE_LOCK.0.store(0, Relaxed);
    run(2, false);
}
