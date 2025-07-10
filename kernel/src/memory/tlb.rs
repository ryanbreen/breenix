//! Translation Lookaside Buffer (TLB) management
//!
//! This module provides safe wrappers around x86_64 TLB flush operations.
//! The TLB caches virtual-to-physical address translations, and must be
//! flushed when page table entries are modified to ensure the CPU sees
//! the updated mappings.