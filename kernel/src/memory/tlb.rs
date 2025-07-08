//! Translation Lookaside Buffer (TLB) management
//!
//! This module provides safe wrappers around x86_64 TLB flush operations.
//! The TLB caches virtual-to-physical address translations, and must be
//! flushed when page table entries are modified to ensure the CPU sees
//! the updated mappings.

// Imports removed - file contains no active code.
// Kept for documentation purposes.

// Note: TLB flush functions removed as they were unused.
// The x86_64 crate provides tlb::flush() and tlb::flush_all() if needed.
// Writing to CR3 automatically flushes the TLB on x86_64.