//! Per-CPU storage for a frame that `aarch64_enter_exception_frame` restores.
//!
//! That routine moves SP onto the frame and loads the registers through SP,
//! so each slot is a stack address for the duration of the restore.
//!
//! Dependency-free so the host tests can check the layout
//! (`tests/arm64_sp_frame_slot.rs`).

use core::mem::MaybeUninit;

/// One CPU's frame slot.
#[repr(C)]
pub struct SpFrameSlot<T>(MaybeUninit<T>);

impl<T> SpFrameSlot<T> {
    /// An all-zero slot, as the static array starts.
    pub const fn zeroed() -> Self {
        Self(MaybeUninit::zeroed())
    }

    /// The frame storage. The caller initialises it before reading through it.
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.0.as_mut_ptr()
    }
}
