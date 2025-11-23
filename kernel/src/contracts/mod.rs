//! Contract-based testing framework for kernel invariants
//!
//! This module provides contracts that verify critical kernel invariants,
//! particularly around page table configuration, stack management, and TSS setup.
//! These contracts catch regressions in CR3 switching, PML4 configuration, and
//! kernel stack accessibility.

#[cfg(feature = "testing")]
pub mod page_table;

#[cfg(feature = "testing")]
pub mod kernel_stack;

#[cfg(feature = "testing")]
pub mod tss;
