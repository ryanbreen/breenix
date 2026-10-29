//! Native owners keep IRQs enabled, but cannot be preempted while holding a lock.
use core::ops::{Deref, DerefMut};

pub(super) struct Mutex<T>(spin::Mutex<T>);
impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self { Self(spin::Mutex::new(value)) }
    pub fn lock(&self) -> Guard<'_, T> {
        crate::per_cpu::preempt_disable();
        Guard { inner: Some(self.0.lock()) }
    }
}

pub(super) struct Guard<'a, T> { inner: Option<spin::MutexGuard<'a, T>> }
impl<T> Deref for Guard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T { self.inner.as_ref().unwrap() }
}
impl<T> DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T { self.inner.as_mut().unwrap() }
}
impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        // Unlock before reenabling scheduling, including on early returns.
        drop(self.inner.take());
        crate::per_cpu::preempt_enable();
    }
}
