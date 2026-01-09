//! Tests for stack allocation bounds checking logic
//!
//! These tests verify that the bounds checking in `kernel/src/memory/stack.rs`
//! correctly prevents allocations that would:
//! 1. Extend to or past the canonical address boundary (0x8000_0000_0000)
//! 2. Overflow when computing the proposed end address
//! 3. Allow allocation AT the boundary (the >= check prevents this)
//!
//! These tests mirror the logic in `check_user_stack_bounds` and
//! `check_kernel_stack_bounds` but run on the host machine rather than
//! within the kernel.

/// Memory layout constants (mirrored from kernel/src/memory/layout.rs)
const USER_STACK_REGION_START: u64 = 0x7FFF_FF00_0000;
const USER_STACK_REGION_END: u64 = 0x8000_0000_0000;
const KERNEL_STACK_ALLOC_START: u64 = 0xFFFF_C900_0000_0000;
const KERNEL_STACK_REGION_END: u64 = 0xFFFF_CA00_0000_0000;

/// Standard stack sizes for testing
const PAGE_SIZE: u64 = 4096;
const STACK_64K: u64 = 64 * 1024; // 64 KiB stack
const STACK_WITH_GUARD: u64 = STACK_64K + PAGE_SIZE; // Stack + guard page

/// Check if a proposed user stack allocation is valid.
///
/// This mirrors the logic in `kernel::memory::stack::check_user_stack_bounds`.
fn check_user_stack_bounds(current_addr: u64, size: u64) -> Result<u64, &'static str> {
    let proposed_end = current_addr.saturating_add(size);

    // Check 1: Would the allocation extend to or past the boundary?
    // We use >= because USER_STACK_REGION_END (0x8000_0000_0000) is non-canonical
    if proposed_end >= USER_STACK_REGION_END {
        return Err("Out of virtual address space for user stacks");
    }

    // Check 2: Did saturating_add wrap around (overflow)?
    if proposed_end < current_addr {
        return Err("Out of virtual address space for user stacks");
    }

    Ok(proposed_end)
}

/// Check if a proposed kernel stack allocation is valid.
///
/// This mirrors the logic in `kernel::memory::stack::check_kernel_stack_bounds`.
fn check_kernel_stack_bounds(current_addr: u64, size: u64) -> Result<u64, &'static str> {
    let proposed_end = current_addr.saturating_add(size);

    if proposed_end >= KERNEL_STACK_REGION_END {
        return Err("Out of virtual address space for kernel stacks");
    }

    if proposed_end < current_addr {
        return Err("Out of virtual address space for kernel stacks");
    }

    Ok(proposed_end)
}

// ============================================================================
// User Stack Bounds Check Tests
// ============================================================================

#[test]
fn test_user_stack_bounds_normal_allocation() {
    // Normal allocation well within bounds should succeed
    let current = USER_STACK_REGION_START;
    let size = STACK_WITH_GUARD;

    let result = check_user_stack_bounds(current, size);
    assert!(result.is_ok(), "Normal allocation should succeed");

    let proposed_end = result.unwrap();
    assert_eq!(proposed_end, current + size);
    assert!(proposed_end < USER_STACK_REGION_END);
}

#[test]
fn test_user_stack_bounds_exactly_at_boundary_fails() {
    // Allocation that would end EXACTLY at the boundary should FAIL
    // because >= check means we can't allocate AT 0x8000_0000_0000
    let remaining_space = USER_STACK_REGION_END - USER_STACK_REGION_START;
    let current = USER_STACK_REGION_START;
    let size = remaining_space; // Would put proposed_end exactly at boundary

    let result = check_user_stack_bounds(current, size);
    assert!(
        result.is_err(),
        "Allocation exactly at boundary should fail"
    );
    assert_eq!(
        result.unwrap_err(),
        "Out of virtual address space for user stacks"
    );
}

#[test]
fn test_user_stack_bounds_one_byte_before_boundary_succeeds() {
    // Allocation that ends one byte BEFORE the boundary should succeed
    let remaining_space = USER_STACK_REGION_END - USER_STACK_REGION_START;
    let current = USER_STACK_REGION_START;
    let size = remaining_space - 1; // One byte less than boundary

    let result = check_user_stack_bounds(current, size);
    assert!(
        result.is_ok(),
        "Allocation one byte before boundary should succeed"
    );

    let proposed_end = result.unwrap();
    assert_eq!(proposed_end, USER_STACK_REGION_END - 1);
}

#[test]
fn test_user_stack_bounds_overflow_protection() {
    // Test that integer overflow is caught
    // Start near the end and try to allocate a huge size
    let current = USER_STACK_REGION_END - PAGE_SIZE; // Near the end
    let size = u64::MAX; // Huge size that would overflow

    let result = check_user_stack_bounds(current, size);
    assert!(result.is_err(), "Overflow should be caught");
}

#[test]
fn test_user_stack_bounds_past_boundary_fails() {
    // Allocation that would extend past the boundary should fail
    let remaining_space = USER_STACK_REGION_END - USER_STACK_REGION_START;
    let current = USER_STACK_REGION_START;
    let size = remaining_space + PAGE_SIZE; // Past the boundary

    let result = check_user_stack_bounds(current, size);
    assert!(result.is_err(), "Allocation past boundary should fail");
}

#[test]
fn test_user_stack_bounds_state_not_modified_on_error() {
    // This test verifies the SEMANTIC requirement that state is not modified on error.
    // Since check_user_stack_bounds is a pure function (no side effects),
    // this test validates that design: the function returns Result without
    // modifying any external state.
    //
    // The actual find_free_virtual_space function only updates NEXT_USER_STACK_ADDR
    // AFTER the bounds check passes, ensuring state consistency on error.

    let current = USER_STACK_REGION_START;
    let size = u64::MAX; // Would overflow

    // Call the function multiple times with invalid input
    let result1 = check_user_stack_bounds(current, size);
    let result2 = check_user_stack_bounds(current, size);

    // Both should return the same error
    assert!(result1.is_err());
    assert!(result2.is_err());
    assert_eq!(result1.unwrap_err(), result2.unwrap_err());
}

#[test]
fn test_user_stack_bounds_consecutive_allocations() {
    // Simulate consecutive allocations until exhaustion
    let mut current = USER_STACK_REGION_START;
    let size = STACK_WITH_GUARD;
    let mut allocation_count = 0;

    loop {
        match check_user_stack_bounds(current, size) {
            Ok(proposed_end) => {
                allocation_count += 1;
                current = proposed_end;
                // Safety: don't run forever
                if allocation_count > 10000 {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    // Verify we made some allocations before exhaustion
    assert!(
        allocation_count > 0,
        "Should have made at least one allocation"
    );

    // Verify we stopped before or at the boundary
    assert!(
        current <= USER_STACK_REGION_END,
        "Current pointer should not exceed boundary"
    );

    // Calculate expected allocations
    let total_space = USER_STACK_REGION_END - USER_STACK_REGION_START;
    let expected_max = total_space / size;
    assert!(
        allocation_count <= expected_max as usize,
        "Should not exceed max possible allocations"
    );
}

// ============================================================================
// Kernel Stack Bounds Check Tests
// ============================================================================

#[test]
fn test_kernel_stack_bounds_normal_allocation() {
    let current = KERNEL_STACK_ALLOC_START;
    let size = STACK_WITH_GUARD;

    let result = check_kernel_stack_bounds(current, size);
    assert!(result.is_ok(), "Normal kernel allocation should succeed");
}

#[test]
fn test_kernel_stack_bounds_at_boundary_fails() {
    let remaining_space = KERNEL_STACK_REGION_END - KERNEL_STACK_ALLOC_START;
    let current = KERNEL_STACK_ALLOC_START;
    let size = remaining_space;

    let result = check_kernel_stack_bounds(current, size);
    assert!(
        result.is_err(),
        "Kernel allocation at boundary should fail"
    );
}

#[test]
fn test_kernel_stack_bounds_one_byte_before_boundary_succeeds() {
    let remaining_space = KERNEL_STACK_REGION_END - KERNEL_STACK_ALLOC_START;
    let current = KERNEL_STACK_ALLOC_START;
    let size = remaining_space - 1;

    let result = check_kernel_stack_bounds(current, size);
    assert!(
        result.is_ok(),
        "Kernel allocation one byte before boundary should succeed"
    );
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[test]
fn test_zero_size_allocation() {
    // Zero-size allocation should technically succeed (proposed_end == current)
    let current = USER_STACK_REGION_START;
    let size = 0;

    let result = check_user_stack_bounds(current, size);
    assert!(result.is_ok(), "Zero-size allocation should succeed");
    assert_eq!(result.unwrap(), current);
}

#[test]
fn test_single_page_allocation() {
    let current = USER_STACK_REGION_START;
    let size = PAGE_SIZE;

    let result = check_user_stack_bounds(current, size);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), current + PAGE_SIZE);
}

#[test]
fn test_max_valid_allocation() {
    // Maximum valid allocation: fills entire region minus 1 byte
    let current = USER_STACK_REGION_START;
    let max_size = USER_STACK_REGION_END - USER_STACK_REGION_START - 1;

    let result = check_user_stack_bounds(current, max_size);
    assert!(result.is_ok(), "Max valid allocation should succeed");
    assert_eq!(result.unwrap(), USER_STACK_REGION_END - 1);
}

#[test]
fn test_boundary_address_is_non_canonical() {
    // Verify that USER_STACK_REGION_END is indeed the canonical boundary
    // 0x8000_0000_0000 is the first non-canonical address in the lower half
    assert_eq!(USER_STACK_REGION_END, 0x8000_0000_0000);

    // Any allocation that would reach this address must fail
    let current = USER_STACK_REGION_END - PAGE_SIZE;
    let size = PAGE_SIZE; // Would end exactly at boundary

    let result = check_user_stack_bounds(current, size);
    assert!(result.is_err(), "Allocation reaching boundary must fail");
}

// ============================================================================
// Fix Verification Tests - These specifically test the fix that changed
// the bounds check to happen BEFORE allocation instead of AFTER
// ============================================================================

#[test]
fn test_fix_bounds_check_before_allocation() {
    // The original bug was that the bounds check happened AFTER incrementing
    // the allocation pointer, meaning on error the pointer was already corrupted.
    //
    // The fix ensures the check happens BEFORE any state modification.
    //
    // This test verifies the semantic: simulate what would happen if we
    // checked AFTER incrementing vs BEFORE incrementing.

    // BEFORE-check approach (the fix):
    fn allocate_before_check(current: &mut u64, size: u64) -> Result<u64, &'static str> {
        let proposed_end = current.saturating_add(size);
        if proposed_end >= USER_STACK_REGION_END || proposed_end < *current {
            // Error: state NOT modified
            return Err("Out of space");
        }
        let addr = *current;
        *current = proposed_end;
        Ok(addr)
    }

    // AFTER-check approach (the bug):
    fn allocate_after_check(current: &mut u64, size: u64) -> Result<u64, &'static str> {
        let addr = *current;
        *current = current.saturating_add(size); // State modified BEFORE check!
        if *current >= USER_STACK_REGION_END || *current < addr {
            // Error: but state is already corrupted!
            return Err("Out of space");
        }
        Ok(addr)
    }

    // Test with an allocation that should fail
    let initial = USER_STACK_REGION_END - PAGE_SIZE;
    let size = PAGE_SIZE * 2; // Would overflow boundary

    // BEFORE-check: state should remain unchanged on error
    let mut state_before = initial;
    let result_before = allocate_before_check(&mut state_before, size);
    assert!(result_before.is_err());
    assert_eq!(
        state_before, initial,
        "BEFORE-check: state should NOT be modified on error"
    );

    // AFTER-check: state gets corrupted even on error
    let mut state_after = initial;
    let result_after = allocate_after_check(&mut state_after, size);
    assert!(result_after.is_err());
    assert_ne!(
        state_after, initial,
        "AFTER-check: state IS modified on error (the bug)"
    );
}

#[test]
fn test_fix_gte_prevents_allocation_at_boundary() {
    // The fix also uses >= (not >) to prevent allocation AT the boundary.
    // This is important because 0x8000_0000_0000 is non-canonical.

    // With >= check (correct):
    fn check_gte(current: u64, size: u64) -> Result<u64, &'static str> {
        let proposed_end = current.saturating_add(size);
        if proposed_end >= USER_STACK_REGION_END {
            return Err("Out of space");
        }
        Ok(proposed_end)
    }

    // With > check (would allow allocation AT boundary):
    fn check_gt(current: u64, size: u64) -> Result<u64, &'static str> {
        let proposed_end = current.saturating_add(size);
        if proposed_end > USER_STACK_REGION_END {
            return Err("Out of space");
        }
        Ok(proposed_end)
    }

    // Test: allocation that ends exactly at boundary
    let remaining = USER_STACK_REGION_END - USER_STACK_REGION_START;

    // >= check correctly rejects this
    let result_gte = check_gte(USER_STACK_REGION_START, remaining);
    assert!(
        result_gte.is_err(),
        ">= check should reject allocation at boundary"
    );

    // > check would incorrectly allow this (the bug we're preventing)
    let result_gt = check_gt(USER_STACK_REGION_START, remaining);
    assert!(
        result_gt.is_ok(),
        "> check would allow allocation at boundary (bug)"
    );
}

#[test]
fn test_saturating_add_prevents_overflow() {
    // The fix uses saturating_add to prevent integer overflow.
    // Without it, a huge size could wrap around and appear valid.

    let current = USER_STACK_REGION_START;
    let huge_size = u64::MAX;

    // With saturating_add: wraps to u64::MAX, which is >= boundary
    let proposed_end = current.saturating_add(huge_size);
    assert_eq!(proposed_end, u64::MAX);
    assert!(proposed_end >= USER_STACK_REGION_END);

    // Without saturating_add (wrapping): could wrap to a "valid" address
    let proposed_end_wrapping = current.wrapping_add(huge_size);
    // This would be current - 1, which could appear valid!
    assert_eq!(proposed_end_wrapping, current - 1);

    // The bounds check catches both cases:
    // 1. proposed_end >= USER_STACK_REGION_END (catches saturated case)
    // 2. proposed_end < current (catches wrapping case)
    let result = check_user_stack_bounds(current, huge_size);
    assert!(result.is_err(), "Overflow must be caught");
}
