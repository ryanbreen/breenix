//! Whole-file lock policy, independent of the scheduler and fcntl record locks.

use alloc::sync::Weak;
use alloc::vec::Vec;

use crate::syscall::errno::{EAGAIN as EWOULDBLOCK, ENOLCK};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Shared,
    Exclusive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Attempt {
    Acquired,
    /// A conversion dropped the old lock. Wake waiters before trying again.
    Released,
    Blocked,
}

pub(crate) struct LockTable<T> {
    holders: Vec<(Weak<T>, Mode)>,
}

impl<T> LockTable<T> {
    pub(crate) const fn new() -> Self {
        Self { holders: Vec::new() }
    }

    pub(crate) fn unlock(&mut self, owner: *const T) -> bool {
        let Some(index) = self.holders.iter().position(|(held, _)| held.as_ptr() == owner) else {
            return false;
        };
        self.holders.swap_remove(index);
        true
    }

    pub(crate) fn try_lock(
        &mut self,
        owner: &Weak<T>,
        mode: Mode,
        nonblocking: bool,
    ) -> Result<Attempt, i32> {
        // A final drop can be on its way to the table. Its lock is no longer live.
        self.holders.retain(|(held, _)| held.strong_count() != 0);
        if let Some(index) = self.holders.iter().position(|(held, _)| held.ptr_eq(owner)) {
            if self.holders[index].1 == mode {
                return Ok(Attempt::Acquired);
            }
            self.holders.swap_remove(index);
            return Ok(Attempt::Released);
        }
        if self.should_wait(owner, mode) {
            return if nonblocking { Err(EWOULDBLOCK) } else { Ok(Attempt::Blocked) };
        }
        self.holders.try_reserve(1).map_err(|_| ENOLCK)?;
        self.holders.push((owner.clone(), mode));
        Ok(Attempt::Acquired)
    }

    /// Recheck before sleeping. A closed description or one that already holds
    /// a lock can make progress without sleeping (including by converting).
    pub(crate) fn should_wait(&self, owner: &Weak<T>, mode: Mode) -> bool {
        owner.strong_count() != 0
            && !self.holders.iter().any(|(held, _)| held.ptr_eq(owner))
            && self.holders.iter().any(|(held, held_mode)| {
                held.strong_count() != 0 && (mode == Mode::Exclusive || *held_mode == Mode::Exclusive)
            })
    }
}
