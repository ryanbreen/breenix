extern crate alloc;

#[allow(dead_code)]
#[path = "../kernel/src/syscall/errno.rs"]
mod kernel_errno;

mod syscall {
    pub mod errno {
        pub use crate::kernel_errno::*;
    }
}

#[path = "../kernel/src/fs/flock_table.rs"]
mod table;

mod flock_table {
    use super::syscall::errno::EAGAIN as EWOULDBLOCK;
    use super::table::{Attempt, LockTable, Mode};
    use std::sync::Arc;

    #[test]
    fn exclusive_excludes_shared_until_unlock() {
        let first = Arc::new(());
        let second = Arc::new(());
        let first_owner = Arc::downgrade(&first);
        let second_owner = Arc::downgrade(&second);
        let mut table = LockTable::new();

        assert_eq!(table.try_lock(&first_owner, Mode::Exclusive, false), Ok(Attempt::Acquired));
        assert_eq!(table.try_lock(&second_owner, Mode::Shared, false), Ok(Attempt::Blocked));
        assert!(table.unlock(first_owner.as_ptr()));
        assert_eq!(table.try_lock(&second_owner, Mode::Shared, false), Ok(Attempt::Acquired));
    }

    #[test]
    fn nb_returns_ewouldblock() {
        let upgrader = Arc::new(());
        let other = Arc::new(());
        let upgrader_owner = Arc::downgrade(&upgrader);
        let other_owner = Arc::downgrade(&other);
        let mut table = LockTable::new();

        assert_eq!(table.try_lock(&upgrader_owner, Mode::Shared, true), Ok(Attempt::Acquired));
        assert_eq!(table.try_lock(&other_owner, Mode::Shared, true), Ok(Attempt::Acquired));
        assert_eq!(table.try_lock(&upgrader_owner, Mode::Exclusive, true), Ok(Attempt::Released));
        assert_eq!(table.try_lock(&upgrader_owner, Mode::Exclusive, true), Err(EWOULDBLOCK));
        assert!(table.unlock(other_owner.as_ptr()));
        assert_eq!(table.try_lock(&other_owner, Mode::Exclusive, true), Ok(Attempt::Acquired));
    }

    #[test]
    fn last_close_releases() {
        let original = Arc::new(());
        let duplicate = original.clone();
        let inherited = original.clone();
        let other = Arc::new(());
        let owner = Arc::downgrade(&original);
        let other_owner = Arc::downgrade(&other);
        let mut table = LockTable::new();

        assert_eq!(table.try_lock(&owner, Mode::Exclusive, false), Ok(Attempt::Acquired));
        drop(original);
        drop(duplicate);
        assert_eq!(table.try_lock(&other_owner, Mode::Exclusive, true), Err(EWOULDBLOCK));
        drop(inherited);
        assert_eq!(table.try_lock(&other_owner, Mode::Exclusive, true), Ok(Attempt::Acquired));
    }

    #[test]
    fn shared_holders_coexist() {
        let first = Arc::new(());
        let second = Arc::new(());
        let first_owner = Arc::downgrade(&first);
        let second_owner = Arc::downgrade(&second);
        let mut table = LockTable::new();

        assert_eq!(table.try_lock(&first_owner, Mode::Shared, false), Ok(Attempt::Acquired));
        assert_eq!(table.try_lock(&second_owner, Mode::Shared, false), Ok(Attempt::Acquired));
        assert_eq!(table.try_lock(&second_owner, Mode::Shared, false), Ok(Attempt::Acquired));
        assert!(!table.should_wait(&second_owner, Mode::Shared));
        assert!(table.unlock(second_owner.as_ptr()));
        assert!(!table.unlock(second_owner.as_ptr()));
        assert!(table.unlock(first_owner.as_ptr()));
    }
}
