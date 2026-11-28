//! The per-CPU frames `aarch64_enter_exception_frame` moves SP onto.
//!
//! `user_cpu::init` sets SCTLR_EL1.SA on each CPU it admits, so a load or
//! store through an SP that is not 16-byte aligned takes an SP alignment
//! fault (Arm ARM, SCTLR_EL1.SA). The routine does `mov sp, x0` and then
//! loads the frame through SP, so each CPU's slot must be 16-byte aligned.
//! With a 264-byte slot (33 registers) the odd-numbered CPUs' slots sit 8
//! bytes off. Those CPUs fault on their first inline-schedule dispatch and
//! again on each exception entry after it, because entry pushes onto the
//! same misaligned SP.
//!
//! Run: scripts/run-structure-tests.sh arm64_sp_frame_slot
#![allow(dead_code)]

#[path = "../kernel/src/arch_impl/aarch64/sp_frame_slot.rs"]
mod sp_frame_slot;

use sp_frame_slot::SpFrameSlot;

/// The kernel's `Aarch64ExceptionFrame` layout: x0-x30, ELR and SPSR, 264
/// bytes at 8-byte alignment. `context_switch.rs` asserts the real type's
/// size is 264 and the entry stubs depend on it.
#[repr(C)]
struct Frame {
    words: [u64; 33],
}

/// ARM64 `constants::MAX_CPUS`.
const CPUS: usize = 8;

static mut SLOTS: [SpFrameSlot<Frame>; CPUS] = [const { SpFrameSlot::zeroed() }; CPUS];

#[test]
fn every_cpu_slot_is_a_valid_stack_pointer() {
    assert_eq!(core::mem::size_of::<Frame>(), 264);
    let misaligned: Vec<usize> = (0..CPUS)
        .filter(|&cpu| {
            // SAFETY: this takes the slot's address and does not read it.
            let address = unsafe { core::ptr::addr_of_mut!(SLOTS[cpu]).cast::<u8>() } as usize;
            address % 16 != 0
        })
        .collect();
    assert!(misaligned.is_empty(), "CPU slots not 16-byte aligned: {misaligned:?}");
}
