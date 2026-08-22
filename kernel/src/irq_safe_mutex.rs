//! Architecture-neutral mutex that masks local interrupts while held.

use core::ops::{Deref, DerefMut};
use spin::{Mutex, MutexGuard};

/// A data-bearing mutex that masks local interrupts for every acquisition.
///
/// The guard releases the inner mutex before restoring the prior interrupt
/// enable state, so a `Drop` body can never orphan the lock by being preempted.
pub struct IrqSafeMutex<T> {
    inner: Mutex<T>,
}

struct IrqState {
    #[cfg(target_arch = "aarch64")]
    daif: u64,
    #[cfg(target_arch = "x86_64")]
    interrupts_were_enabled: bool,
}

impl IrqState {
    #[inline(always)]
    fn save_and_mask() -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            let daif: u64;
            unsafe {
                core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
                core::arch::asm!("msr daifset, #3", options(nomem, nostack));
            }
            Self { daif }
        }
        #[cfg(target_arch = "x86_64")]
        {
            let interrupts_were_enabled = crate::arch_interrupts_enabled();
            unsafe {
                crate::arch_disable_interrupts();
            }
            Self {
                interrupts_were_enabled,
            }
        }
    }

    #[inline(always)]
    fn restore(&self) {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("msr daif, {}", in(reg) self.daif, options(nomem, nostack));
        }
        #[cfg(target_arch = "x86_64")]
        if self.interrupts_were_enabled {
            unsafe {
                crate::arch_enable_interrupts();
            }
        }
    }
}

impl<T> IrqSafeMutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            inner: Mutex::new(value),
        }
    }

    #[inline(always)]
    pub fn lock(&self) -> IrqSafeMutexGuard<'_, T> {
        let irq_state = IrqState::save_and_mask();
        IrqSafeMutexGuard {
            inner: Some(self.inner.lock()),
            irq_state,
        }
    }

    #[inline(always)]
    pub fn try_lock(&self) -> Option<IrqSafeMutexGuard<'_, T>> {
        let irq_state = IrqState::save_and_mask();
        match self.inner.try_lock() {
            Some(inner) => Some(IrqSafeMutexGuard {
                inner: Some(inner),
                irq_state,
            }),
            None => {
                irq_state.restore();
                None
            }
        }
    }
}

/// Guard that releases its mutex before restoring the saved interrupt state.
pub struct IrqSafeMutexGuard<'a, T> {
    inner: Option<MutexGuard<'a, T>>,
    irq_state: IrqState,
}

impl<T> Deref for IrqSafeMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.inner.as_deref().expect("IRQ-safe mutex guard held")
    }
}

impl<T> DerefMut for IrqSafeMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner
            .as_deref_mut()
            .expect("IRQ-safe mutex guard held")
    }
}

impl<T> Drop for IrqSafeMutexGuard<'_, T> {
    fn drop(&mut self) {
        drop(self.inner.take());
        self.irq_state.restore();
    }
}
