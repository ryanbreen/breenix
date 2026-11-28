//! Per-CPU storage for a frame that `aarch64_enter_exception_frame` restores.
//!
//! That routine moves SP onto the frame and loads the registers through SP,
//! so each slot is a stack address for the duration of the restore. With
//! SCTLR_EL1.SA set (`user_cpu::init`), an SP-based access through an SP
//! that is not 16-byte aligned takes an SP alignment fault (Arm ARM,
//! SCTLR_EL1.SA; AAPCS64 keeps SP 16-byte aligned). The exception entry
//! that fault takes pushes onto the same SP and faults again, and so on:
//! CPUs 1 and 3 of a 4-CPU QEMU boot stopped there (#3257's SA change with
//! the 264-byte slots this type replaces).
//!
//! Dependency-free so the host tests can check the layout
//! (`tests/arm64_sp_frame_slot.rs`).

use core::mem::MaybeUninit;

/// One CPU's frame slot. The alignment also rounds the size up to a
/// multiple of 16, so each element of a slot array is 16-byte aligned.
#[repr(C, align(16))]
pub struct SpFrameSlot<T>(MaybeUninit<T>);

impl<T> SpFrameSlot<T> {
    /// A zero-filled slot, which is how the static array starts.
    pub const fn zeroed() -> Self {
        Self(MaybeUninit::zeroed())
    }

    /// The frame storage. The caller initialises it before reading through it.
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.0.as_mut_ptr()
    }
}
