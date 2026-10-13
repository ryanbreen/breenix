//! Fallible waiter capacity, before any blocked state is published.
use alloc::collections::VecDeque;
pub(crate) fn reserve_one<T>(waiters:&mut VecDeque<T>)->bool { waiters.try_reserve(1).is_ok() }
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reservation_does_not_publish_a_waiter() {
        let mut waiters=VecDeque::<u64>::new();
        assert!(reserve_one(&mut waiters));assert!(waiters.is_empty());
        waiters.push_back(20);assert_eq!(waiters.front(),Some(&20));
    }
}
