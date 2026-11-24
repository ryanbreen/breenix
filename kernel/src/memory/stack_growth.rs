use core::ops::Range;

/// Virtual space reserved for a stack, with pages mapped on demand.
pub const MAX_USER_STACK_SIZE: u64 = 64 * 1024 * 1024;

/// Decide whether a page is within the stack and whether any new pages can
/// be charged and mapped. Only growth needs the address-space and VMA checks.
pub(crate) fn permits_stack_growth(
    stack: Range<u64>,
    page: u64,
    stack_limit: u64,
    address_space_limit: u64,
    mapped_bytes: impl FnOnce() -> u64,
    mut vmas: impl Iterator<Item = Range<u64>>,
) -> bool {
    if page >= stack.end || stack.end - page > stack_limit.min(MAX_USER_STACK_SIZE) {
        return false;
    }
    page >= stack.start
        || (mapped_bytes().saturating_add(stack.start - page) <= address_space_limit
            && !vmas.any(|vma| vma.start < stack.start && vma.end > page))
}
