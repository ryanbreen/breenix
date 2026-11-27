//! Task-context exclusion for operations that can block on I/O.
use super::wait::{wait_on, IpcDeadline, WaitEnd};
use crate::task::waitqueue::WaitQueueHead;
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct SleepMutex<T> {
    busy: AtomicBool,
    waiters: WaitQueueHead,
    value: UnsafeCell<T>,
}

// The successful acquire gives exactly one guard access to the value.
unsafe impl<T: Send> Sync for SleepMutex<T> {}

#[must_use]
pub(crate) struct SleepGuard<'a, T> {
    lock: &'a SleepMutex<T>,
    value: PhantomData<&'a mut T>,
}

impl<T> SleepMutex<T> {
    pub(crate) fn new(value: T) -> Self {
        Self {
            busy: AtomicBool::new(false),
            waiters: WaitQueueHead::new(),
            value: UnsafeCell::new(value),
        }
    }

    /// Call from a sleepable task with scheduler and process-table guards released.
    pub(crate) fn enter(&self) -> Result<SleepGuard<'_, T>, u64> {
        loop {
            if self
                .busy
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return Ok(SleepGuard {
                    lock: self,
                    value: PhantomData,
                });
            }
            match wait_on(&self.waiters, &IpcDeadline::Never, || {
                self.busy.load(Ordering::Acquire)
            }) {
                WaitEnd::Interrupted(error) => return Err(error),
                WaitEnd::Woken | WaitEnd::TimedOut => {}
            }
        }
    }
}

impl<T> Deref for SleepGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: only the live guard can access the value.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SleepGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: only the live guard can access the value.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SleepGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.busy.store(false, Ordering::Release);
        self.lock.waiters.wake_up();
    }
}
