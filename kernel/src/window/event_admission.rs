//! Lossless admission of compositor input under B48.
extern crate alloc;
use alloc::collections::VecDeque;

/// Refuse a full ring without changing any queued event.
pub fn push<T>(queue: &mut VecDeque<T>, capacity: usize, event: T) -> bool {
    if queue.len() >= capacity { return false; }
    queue.push_back(event);
    true
}
