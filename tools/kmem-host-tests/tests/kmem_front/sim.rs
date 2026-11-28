//! A mutex under a fixed schedule. Another simulated CPU can hold it for a set
//! number of polls; each poll or blocking acquire of a held lock is one wait.
use std::cell::{Cell, RefCell, RefMut};

/// Waits after which the holder is taken to be unable to release the lock
/// while this CPU waits (it is the caller itself, or it is stopped).
const DEADLOCK_WAITS: u32 = 1_000;

pub struct SimLock<T> {
    value: RefCell<T>,
    held: Cell<bool>,
    release_after: Cell<u32>,
    try_locks: Cell<u32>,
    waits: Cell<u32>,
}

impl<T> SimLock<T> {
    pub fn new(value: T) -> Self {
        Self {
            value: RefCell::new(value),
            held: Cell::new(false),
            release_after: Cell::new(0),
            try_locks: Cell::new(0),
            waits: Cell::new(0),
        }
    }

    /// Another CPU takes the lock and releases it after `polls` waits by this
    /// CPU. With no count the holder keeps it while this CPU waits.
    pub fn hold_elsewhere(&self, polls: Option<u32>) {
        self.held.set(true);
        self.release_after.set(polls.unwrap_or(u32::MAX));
    }

    pub fn try_locks(&self) -> u32 {
        self.try_locks.get()
    }

    pub fn waits(&self) -> u32 {
        self.waits.get()
    }

    pub fn try_lock(&self) -> Option<RefMut<'_, T>> {
        self.try_locks.set(self.try_locks.get() + 1);
        (!self.held.get()).then(|| self.value.borrow_mut())
    }

    pub fn is_locked(&self) -> bool {
        if self.held.get() {
            self.wait();
        }
        self.held.get()
    }

    pub fn lock(&self) -> RefMut<'_, T> {
        while self.held.get() {
            self.wait();
        }
        self.value.borrow_mut()
    }

    fn wait(&self) {
        let waits = self.waits.get() + 1;
        self.waits.set(waits);
        assert!(
            waits < DEADLOCK_WAITS,
            "waited {waits} times on a lock whose holder cannot release it"
        );
        let left = self.release_after.get().saturating_sub(1);
        self.release_after.set(left);
        if left == 0 {
            self.held.set(false);
        }
    }
}
