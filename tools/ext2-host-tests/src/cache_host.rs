//! Host counterpart of cache_kernel: real blocking waits without a scheduler.
use super::cache::{bound_for_memory, BufferCache, Wait};
use std::sync::{Condvar, Mutex, OnceLock};

pub struct HostWait {
    generation: Mutex<u64>,
    changed: Condvar,
}
impl Wait for HostWait {
    fn generation(&self) -> u64 {
        *self.generation.lock().unwrap()
    }
    fn wait(&self, observed: u64) {
        let mut generation = self.generation.lock().unwrap();
        while *generation == observed {
            generation = self.changed.wait(generation).unwrap();
        }
    }
    fn wake_all(&self) {
        *self.generation.lock().unwrap() += 1;
        self.changed.notify_all();
    }
}
pub type SystemCache = BufferCache<HostWait>;
static CACHE: OnceLock<SystemCache> = OnceLock::new();
pub fn system_cache() -> &'static SystemCache {
    CACHE.get_or_init(|| {
        SystemCache::with_capacity(
            HostWait {
                generation: Mutex::new(0),
                changed: Condvar::new(),
            },
            bound_for_memory(32 * 1024 * 1024),
        )
    })
}
// The host fixture has no process-manager lock.
pub fn assert_io_context() {}
