//! Fixed-ring admission shared by the legacy input syscall and its #2163 host test.

/// Refuse a full ring without changing its queued events or either cursor.
pub fn push<T: Copy>(ring: &mut [T], head: &mut usize, tail: usize, event: T) -> bool {
    let next = (*head + 1) % ring.len();
    if next == tail { return false; }
    ring[*head] = event;
    *head = next;
    true
}
