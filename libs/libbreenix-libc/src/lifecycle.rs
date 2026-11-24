//! ELF gABI §8.6 initialization and termination order, and the Itanium C++
//! ABI's DSO object destruction API (currently §3.3.6).
//!
//! The runners take their arrays, lists and final actions from the caller.
//! They do not depend on the linker, allocator, or pthread runtime.

use core::cell::UnsafeCell;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

pub(crate) type ArrayEntry = unsafe extern "C" fn();

pub(crate) unsafe fn run_main<T>(
    preinit: &[ArrayEntry],
    init: &[ArrayEntry],
    main: impl FnOnce() -> T,
) -> T {
    for &entry in preinit.iter().chain(init) {
        // SAFETY: the caller supplies callable entries from the two arrays.
        unsafe { entry() };
    }
    main()
}

pub(crate) unsafe fn run_exit(
    next: impl FnMut() -> Option<Handler>,
    fini: &[ArrayEntry],
    flush: impl FnOnce(),
) {
    // SAFETY: the caller supplies registered callbacks and their arguments.
    unsafe { run_handlers(next) };
    for &entry in fini.iter().rev() {
        // SAFETY: the caller supplies callable entries from the fini array.
        unsafe { entry() };
    }
    flush();
}

pub(crate) unsafe fn run_thread_exit(
    next: impl FnMut() -> Option<Handler>,
    key_destructors: impl FnOnce(),
) {
    // SAFETY: the caller supplies the exiting thread's registered callbacks.
    unsafe { run_handlers(next) };
    key_destructors();
}

pub(crate) unsafe fn run_handlers(mut next: impl FnMut() -> Option<Handler>) {
    // Pull each entry separately so a callback can change the remaining list.
    while let Some(handler) = next() {
        match handler {
            Handler::C(function) => function(),
            Handler::Cxa {
                function, argument, ..
            } => {
                // SAFETY: the callback and its argument were registered together.
                unsafe { function(argument) };
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Handler {
    C(extern "C" fn()),
    Cxa {
        function: unsafe extern "C" fn(*mut u8),
        argument: *mut u8,
        dso: *mut u8,
    },
}

impl Handler {
    fn matches(self, handle: *mut u8) -> bool {
        handle.is_null() || matches!(self, Self::Cxa { dso, .. } if dso == handle)
    }
}

pub(crate) struct HandlerNode {
    handler: Handler,
    next: AtomicPtr<HandlerNode>,
}

impl HandlerNode {
    pub(crate) const fn new(handler: Handler) -> Self {
        Self {
            handler,
            next: AtomicPtr::new(null_mut()),
        }
    }

    pub(crate) fn handler(&self) -> Handler {
        self.handler
    }
}

/// Owns each node from insertion until it is unlinked. The caller provides and
/// reclaims storage so the same list can run with either guest or host allocation.
pub(crate) struct HandlerList {
    head: AtomicPtr<HandlerNode>,
}

impl HandlerList {
    pub(crate) const fn new() -> Self {
        Self {
            head: AtomicPtr::new(null_mut()),
        }
    }

    pub(crate) fn push(&mut self, node: &'static mut HandlerNode) {
        node.next
            .store(self.head.load(Ordering::Relaxed), Ordering::Relaxed);
        self.head.store(node, Ordering::Release);
    }

    /// Unlink the newest matching registration before giving it to the caller.
    /// A single pointer store publishes the unlink, so a fork sees a complete chain.
    pub(crate) fn take(&mut self, dso: *mut u8) -> Option<&'static mut HandlerNode> {
        let mut link = &self.head;
        loop {
            let node = link.load(Ordering::Acquire);
            // SAFETY: the list owns each non-null node reachable from its head.
            let entry = unsafe { node.as_ref() }?;
            if entry.handler.matches(dso) {
                link.store(entry.next.load(Ordering::Relaxed), Ordering::Release);
                // SAFETY: the node is unlinked and the list had exclusive ownership.
                return Some(unsafe { &mut *node });
            }
            link = &entry.next;
        }
    }
}

pub(crate) struct ProcessHandlers {
    busy: AtomicBool,
    list: UnsafeCell<HandlerList>,
}

// The process lock serializes list access. User callbacks run outside the lock.
unsafe impl Sync for ProcessHandlers {}

impl ProcessHandlers {
    pub(crate) const fn new() -> Self {
        Self {
            busy: AtomicBool::new(false),
            list: UnsafeCell::new(HandlerList::new()),
        }
    }

    pub(crate) fn push(&self, node: &'static mut HandlerNode) {
        self.with_list(|list| list.push(node));
    }

    pub(crate) fn take(&self, dso: *mut u8) -> Option<&'static mut HandlerNode> {
        self.with_list(|list| list.take(dso))
    }

    fn with_list<T>(&self, op: impl FnOnce(&mut HandlerList) -> T) -> T {
        while self.busy.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
        // SAFETY: the acquired lock gives exclusive access to the list.
        let result = op(unsafe { &mut *self.list.get() });
        // SAFETY: this operation acquired the lock above.
        unsafe { self.unlock() };
        result
    }

    /// The caller holds the lock, or is the only thread in a freshly forked
    /// child with an inherited copy of the list.
    pub(crate) unsafe fn unlock(&self) {
        self.busy.store(false, Ordering::Release);
    }
}
