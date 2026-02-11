//! Spinlock implementation with preempt_count integration
//!
//! This matches Linux kernel spinlock semantics where acquiring
//! a spinlock disables preemption via preempt_count.

use core::sync::atomic::{AtomicBool, Ordering};
use core::hint::spin_loop;

/// A simple spinlock that integrates with preempt_count
/// 
/// When a spinlock is acquired, it increments preempt_count to disable
/// preemption. When released, it decrements preempt_count and may trigger
/// scheduling if needed.
pub struct SpinLock {
    locked: AtomicBool,
}

impl SpinLock {
    /// Create a new unlocked spinlock
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
        }
    }

    /// Acquire the spinlock
    /// 
    /// This disables preemption by incrementing preempt_count,
    /// then spins until the lock is acquired.
    pub fn lock(&self) -> SpinLockGuard<'_> {
        // Disable preemption FIRST before trying to acquire the lock
        // This prevents being preempted while holding the lock
        crate::per_cpu::preempt_disable();
        
        // Now spin until we acquire the lock
        while self.locked.compare_exchange_weak(
            false,
            true,
            Ordering::Acquire,
            Ordering::Relaxed
        ).is_err() {
            // Hint to the CPU that we're spinning
            spin_loop();
        }
        
        SpinLockGuard { lock: self }
    }

    /// Try to acquire the spinlock without blocking
    ///
    /// Returns Some(guard) if successful, None if the lock is held
    #[allow(dead_code)]
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_>> {
        // Disable preemption first
        crate::per_cpu::preempt_disable();
        
        // Try to acquire the lock
        if self.locked.compare_exchange(
            false,
            true,
            Ordering::Acquire,
            Ordering::Relaxed
        ).is_ok() {
            Some(SpinLockGuard { lock: self })
        } else {
            // Failed to acquire, re-enable preemption
            crate::per_cpu::preempt_enable();
            None
        }
    }

    /// Release the spinlock (internal use only)
    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
        // Re-enable preemption, which may trigger scheduling
        crate::per_cpu::preempt_enable();
    }
}

/// RAII guard for spinlock
/// 
/// When dropped, releases the lock and re-enables preemption
pub struct SpinLockGuard<'a> {
    lock: &'a SpinLock,
}

impl<'a> Drop for SpinLockGuard<'a> {
    fn drop(&mut self) {
        self.lock.unlock();
    }
}

// SpinLock is Sync because it provides its own synchronization
unsafe impl Sync for SpinLock {}
// SpinLock is Send because it doesn't contain any !Send types
unsafe impl Send for SpinLock {}

/// A spinlock that also disables interrupts
///
/// This is used for locks that may be taken in interrupt context
#[allow(dead_code)]
pub struct SpinLockIrq {
    lock: SpinLock,
}

impl SpinLockIrq {
    /// Create a new unlocked spinlock
    #[allow(dead_code)]
    pub const fn new() -> Self {
        Self {
            lock: SpinLock::new(),
        }
    }

    /// Acquire the spinlock with interrupts disabled
    #[allow(dead_code)]
    pub fn lock(&self) -> SpinLockIrqGuard<'_> {
        // Save interrupt state and disable interrupts
        let was_enabled = crate::arch_interrupts_enabled();
        unsafe { crate::arch_disable_interrupts(); }

        // Now acquire the regular spinlock (which disables preemption)
        let _guard = self.lock.lock();

        SpinLockIrqGuard {
            lock: &self.lock,
            irq_was_enabled: was_enabled,
        }
    }
}

/// RAII guard for IRQ spinlock
#[allow(dead_code)]
pub struct SpinLockIrqGuard<'a> {
    lock: &'a SpinLock,
    irq_was_enabled: bool,
}

impl<'a> Drop for SpinLockIrqGuard<'a> {
    fn drop(&mut self) {
        // Release the lock (and re-enable preemption)
        self.lock.unlock();

        // Restore interrupt state
        if self.irq_was_enabled {
            unsafe { crate::arch_enable_interrupts(); }
        }
    }
}

// SpinLockIrq is Sync and Send for the same reasons as SpinLock
unsafe impl Sync for SpinLockIrq {}
unsafe impl Send for SpinLockIrq {}

/// Test that spinlock acquisition disables preemption
pub fn test_spinlock_preemption() {
        log::info!("Testing spinlock preemption integration...");
        
        let lock = SpinLock::new();
        let initial_count = crate::per_cpu::preempt_count();
        log::info!("Initial preempt_count: {:#x}", initial_count);
        
        // Acquire the lock
        let guard = lock.lock();
        let with_lock_count = crate::per_cpu::preempt_count();
        log::info!("With spinlock held: {:#x}", with_lock_count);
        
        // Preempt count should be incremented
        assert_eq!(with_lock_count, initial_count + 1, "Spinlock should disable preemption");
        
        // Release the lock
        drop(guard);
        let after_release = crate::per_cpu::preempt_count();
        log::info!("After spinlock release: {:#x}", after_release);
        
        // Preempt count should be back to initial
        assert_eq!(after_release, initial_count, "Spinlock release should re-enable preemption");
        
        log::info!("✅ Spinlock preemption integration test passed");
}