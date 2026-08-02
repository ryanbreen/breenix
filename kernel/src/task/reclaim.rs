//! Deferred process resource reclamation.

use alloc::alloc::{alloc, Layout};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::memory::process_memory::ProcessPageTable;
use crate::memory::stack::GuardedStack;
use crate::process::ProcessId;

/// Resources reserved at process birth and filled by the one-shot exit commit.
pub struct ProcessGrave {
    pub pid: ProcessId,
    pub exit_code: i32,
    pub page_table: Option<Box<ProcessPageTable>>,
    pub old_page_tables: Vec<Box<ProcessPageTable>>,
    pub stack: Option<Box<GuardedStack>>,
    #[cfg(target_arch = "aarch64")]
    pub fence: super::scheduler::RetirementFence,
    pub queued_at_ns: u64,
    pub warned: bool,
    pub(crate) next: *mut ProcessGrave,
}

// The intrusive next pointer is owned exclusively by the graveyard stack or by
// the reclaimer that detached it. No producer dereferences another node.
unsafe impl Send for ProcessGrave {}

static GRAVEYARD: AtomicPtr<ProcessGrave> = AtomicPtr::new(core::ptr::null_mut());

impl ProcessGrave {
    /// Allocate the grave fallibly so process creation can report ENOMEM.
    pub fn try_new(pid: ProcessId) -> Option<Box<Self>> {
        let layout = Layout::new::<Self>();
        let ptr = unsafe { alloc(layout).cast::<Self>() };
        if ptr.is_null() {
            return None;
        }

        unsafe {
            ptr.write(Self {
                pid,
                exit_code: 0,
                page_table: None,
                old_page_tables: Vec::new(),
                stack: None,
                #[cfg(target_arch = "aarch64")]
                fence: super::scheduler::RetirementFence::invalid(),
                queued_at_ns: 0,
                warned: false,
                next: core::ptr::null_mut(),
            });
            Some(Box::from_raw(ptr))
        }
    }
}

/// Publish one preallocated grave on the intrusive Treiber stack.
pub fn push_grave(grave: Box<ProcessGrave>) {
    let grave = Box::into_raw(grave);
    let mut head = GRAVEYARD.load(Ordering::Relaxed);
    loop {
        unsafe {
            (*grave).next = head;
        }
        match GRAVEYARD.compare_exchange_weak(head, grave, Ordering::Release, Ordering::Relaxed) {
            Ok(_) => return,
            Err(observed) => head = observed,
        }
    }
}

/// Detach the entire graveyard for one reclaimer pass.
pub fn take_all_graves() -> *mut ProcessGrave {
    GRAVEYARD.swap(core::ptr::null_mut(), Ordering::Acquire)
}
