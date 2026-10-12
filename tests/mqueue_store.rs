//! Exercise the POSIX message queue store and its argument checks on the host.
extern crate alloc;
mod syscall {
    pub mod errno {
        pub const EACCES: i32 = 13;
        pub const EINVAL: i32 = 22;
        pub const ENAMETOOLONG: i32 = 36;
        pub const EMSGSIZE: i32 = 90;
    }
}
#[path = "../kernel/src/ipc/mqueue/store.rs"]
mod store;
use store::*;

const EACCES: u64 = 13;
const EINVAL: u64 = 22;
const ENAMETOOLONG: u64 = 36;
const EMSGSIZE: u64 = 90;

fn attr(maxmsg: i64, msgsize: i64) -> MqAttr {
    MqAttr { mq_maxmsg: maxmsg, mq_msgsize: msgsize, ..MqAttr::default() }
}

#[test]
fn mq_attr_has_the_linux_layout() {
    assert_eq!(core::mem::size_of::<MqAttr>(), 64);
    let a = MqAttr::default();
    let base = &a as *const MqAttr as usize;
    assert_eq!(&a.mq_flags as *const i64 as usize - base, 0);
    assert_eq!(&a.mq_maxmsg as *const i64 as usize - base, 8);
    assert_eq!(&a.mq_msgsize as *const i64 as usize - base, 16);
    assert_eq!(&a.mq_curmsgs as *const i64 as usize - base, 24);
}

#[test]
fn names_are_checked_as_linux_checks_them() {
    assert_eq!(check_name(b"queue"), Ok(()));
    assert_eq!(check_name(&[b'q'; NAME_MAX]), Ok(()));
    assert_eq!(check_name(&[b'q'; NAME_MAX + 1]), Err(ENAMETOOLONG));
    assert_eq!(check_name(b""), Err(EACCES));
    assert_eq!(check_name(b"."), Err(EACCES));
    assert_eq!(check_name(b".."), Err(EACCES));
    assert_eq!(check_name(b"a/b"), Err(EACCES));
    assert_eq!(check_name(b"/a"), Err(EACCES));
}

#[test]
fn no_attributes_gives_the_defaults() {
    assert_eq!(check_attr(None, false), Ok((10, 8192)));
    assert_eq!(check_attr(None, true), Ok((10, 8192)));
}

#[test]
fn attributes_of_zero_or_less_are_einval() {
    for (maxmsg, msgsize) in [(0, 64), (4, 0), (-1, 64), (4, -1)] {
        assert_eq!(check_attr(Some(&attr(maxmsg, msgsize)), true), Err(EINVAL), "{maxmsg} {msgsize}");
    }
}

#[test]
fn attributes_above_the_callers_cap_are_einval() {
    assert_eq!(check_attr(Some(&attr(10, 8192)), false), Ok((10, 8192)));
    assert_eq!(check_attr(Some(&attr(11, 64)), false), Err(EINVAL));
    assert_eq!(check_attr(Some(&attr(4, 8193)), false), Err(EINVAL));
    assert_eq!(check_attr(Some(&attr(65536, 16 << 20)), true), Ok((65536, 16 << 20)));
    assert_eq!(check_attr(Some(&attr(65537, 64)), true), Err(EINVAL));
    assert_eq!(check_attr(Some(&attr(4, (16 << 20) + 1)), true), Err(EINVAL));
}

#[test]
fn sends_check_priority_before_size() {
    assert_eq!(check_send(64, MQ_PRIO_MAX - 1, 64), Ok(()));
    assert_eq!(check_send(0, 0, 64), Ok(()));
    assert_eq!(check_send(64, MQ_PRIO_MAX, 64), Err(EINVAL));
    assert_eq!(check_send(65, 0, 64), Err(EMSGSIZE));
    assert_eq!(check_send(65, MQ_PRIO_MAX, 64), Err(EINVAL));
}

#[test]
fn a_receive_buffer_shorter_than_msgsize_is_emsgsize() {
    assert_eq!(check_receive(64, 64), Ok(()));
    assert_eq!(check_receive(4096, 64), Ok(()));
    assert_eq!(check_receive(63, 64), Err(EMSGSIZE));
}

#[test]
fn setattr_changes_only_o_nonblock() {
    let mut a = attr(99, 99);
    a.mq_curmsgs = 99;
    assert_eq!(check_setattr(&a), Ok(false));
    a.mq_flags = O_NONBLOCK;
    assert_eq!(check_setattr(&a), Ok(true));
    a.mq_flags = O_NONBLOCK | 0o2;
    assert_eq!(check_setattr(&a), Err(EINVAL));
}

#[test]
fn messages_leave_highest_priority_first_and_oldest_first_within_one() {
    let mut q = MessageStore::new(8);
    for (msg, prio) in [("a1", 1), ("b5", 5), ("c1", 1), ("d5", 5), ("e0", 0), ("f31", 31)] {
        q.push(prio, msg.as_bytes().to_vec()).unwrap();
    }
    assert_eq!(q.len(), 6);
    let mut order = Vec::new();
    while let Some((msg, prio)) = q.pop() {
        order.push(format!("{}@{prio}", String::from_utf8(msg).unwrap()));
    }
    assert_eq!(order, ["f31@31", "b5@5", "d5@5", "a1@1", "c1@1", "e0@0"]);
    assert!(q.is_empty());
}

#[test]
fn a_full_queue_refuses_a_message_and_hands_it_back() {
    let mut q = MessageStore::new(2);
    q.push(0, b"1".to_vec()).unwrap();
    q.push(0, b"2".to_vec()).unwrap();
    assert!(q.is_full());
    assert_eq!(q.push(7, b"3".to_vec()), Err(b"3".to_vec()));
    assert_eq!(q.len(), 2);
    assert_eq!(q.pop(), Some((b"1".to_vec(), 0)));
    assert!(!q.is_full());
    q.push(7, b"3".to_vec()).unwrap();
    assert_eq!(q.pop(), Some((b"3".to_vec(), 7)));
    assert_eq!(q.pop(), Some((b"2".to_vec(), 0)));
    assert_eq!(q.pop(), None);
}

#[test]
fn empty_messages_are_messages() {
    let mut q = MessageStore::new(1);
    q.push(3, Vec::new()).unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(q.pop(), Some((Vec::new(), 3)));
    assert!(q.is_empty());
}
