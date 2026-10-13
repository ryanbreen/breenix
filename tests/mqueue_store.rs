//! Exercise the POSIX message queue store and its argument checks on the host.
extern crate alloc;
mod syscall {
    pub mod errno {
        pub const ENOENT: i32 = 2;
        pub const ENOMEM: i32 = 12;
        pub const EACCES: i32 = 13;
        pub const EINVAL: i32 = 22;
        pub const EMFILE: i32 = 24;
        pub const ENOSPC: i32 = 28;
        pub const ENAMETOOLONG: i32 = 36;
        pub const EMSGSIZE: i32 = 90;
    }
}
#[path = "../kernel/src/ipc/mqueue/store.rs"]
mod store;
use store::*;

const ENOENT: u64 = 2;
const EACCES: u64 = 13;
const EINVAL: u64 = 22;
const EMFILE: u64 = 24;
const ENOSPC: u64 = 28;
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
    // mq_open("/") and mq_unlink("/") reach the kernel as the empty name.
    assert_eq!(check_name(b""), Err(ENOENT));
    assert_eq!(check_name(b"."), Err(EACCES));
    assert_eq!(check_name(b".."), Err(EACCES));
    assert_eq!(check_name(b"..."), Ok(()));
    assert_eq!(check_name(b"a/b"), Err(EACCES));
    assert_eq!(check_name(b"/a"), Err(EACCES));
}

#[test]
fn names_are_byte_strings() {
    assert_eq!(check_name(b"\xff"), Ok(()));
    assert_eq!(check_name(b"\xc3\x28queue"), Ok(()));
}

#[test]
fn a_long_name_with_a_slash_is_eacces_before_enametoolong() {
    let mut long = vec![b'q'; NAME_MAX + 10];
    assert_eq!(check_name(&long), Err(ENAMETOOLONG));
    long[NAME_MAX + 5] = b'/';
    assert_eq!(check_name(&long), Err(EACCES));
    // As the kernel scans it: only the first NAME_MAX bytes are kept.
    assert_eq!(check_scanned(&long[..NAME_MAX], long.len(), true), Err(EACCES));
    assert_eq!(check_scanned(&long[..NAME_MAX], long.len(), false), Err(ENAMETOOLONG));
    assert_eq!(check_scanned(b".", 1, false), Err(EACCES));
    // The kernel looks PATH_MAX bytes far for the terminator.
    assert!(NAME_MAX < PATH_MAX);
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
fn timeouts_are_checked_whole_and_first() {
    assert_eq!(check_timeout(0, 0), Ok(0));
    assert_eq!(check_timeout(1, 999_999_999), Ok(1_999_999_999));
    assert_eq!(check_timeout(-1, 0), Err(EINVAL));
    assert_eq!(check_timeout(1, -1), Err(EINVAL));
    assert_eq!(check_timeout(1, 1_000_000_000), Err(EINVAL));
    assert_eq!(check_timeout(i64::MAX, 999_999_999), Ok(u64::MAX));
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
fn many_messages_of_one_priority_stay_in_order() {
    let mut q = MessageStore::new(1000);
    for i in 0..1000u32 {
        q.push(7, i.to_le_bytes().to_vec()).unwrap();
    }
    for i in 0..1000u32 {
        assert_eq!(q.pop(), Some((i.to_le_bytes().to_vec(), 7)));
    }
}

#[test]
fn a_full_queue_refuses_a_message_and_hands_it_back() {
    let mut q = MessageStore::new(2);
    q.push(0, b"1".to_vec()).unwrap();
    q.push(0, b"2".to_vec()).unwrap();
    assert!(q.is_full());
    assert_eq!(q.push(7, b"3".to_vec()), Err(Refused::Full(b"3".to_vec())));
    assert_eq!(q.len(), 2);
    assert_eq!(q.pop(), Some((b"1".to_vec(), 0)));
    assert!(!q.is_full());
    q.push(7, b"3".to_vec()).unwrap();
    assert_eq!(q.pop(), Some((b"3".to_vec(), 7)));
    assert_eq!(q.pop(), Some((b"2".to_vec(), 0)));
    assert_eq!(q.pop(), None);
}

#[test]
fn a_name_already_in_the_table_is_not_inserted_again() {
    let mut table: NameTable<u32> = NameTable::new();
    assert_eq!(table.insert(b"a", 1), Ok(()));
    assert_eq!(table.insert(b"a", 2), Err((EINVAL, 2)));
    assert_eq!(table.get(b"a"), Some(&1));
}

#[test]
fn empty_messages_are_messages() {
    let mut q = MessageStore::new(1);
    q.push(3, Vec::new()).unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(q.pop(), Some((Vec::new(), 3)));
    assert!(q.is_empty());
}

/// Ranks by thread ID, as the scheduler would report them.
fn ranks(table: &'static [(u64, u8)]) -> impl Fn(u64) -> u8 {
    move |id| table.iter().find(|e| e.0 == id).map_or(0, |e| e.1)
}

const UNRANKED: &[(u64, u8)] = &[];

#[test]
fn the_highest_ranked_waiter_that_waited_longest_is_served_first() {
    let mut w = WaitList::new();
    for id in [10, 11, 12, 13] {
        w.add(id, None).unwrap();
    }
    let rank = ranks(&[(10, 1), (11, 50), (12, 50), (13, 1)]);
    let mut order = Vec::new();
    while let Some(next) = w.next_waiting(&rank) {
        next.done = true;
        order.push(next.id);
    }
    assert_eq!(order, [11, 12, 10, 13]);
    assert!(!w.any_waiting());
    assert!(w.get(12).is_some_and(|e| e.done));
    assert_eq!(w.remove(12).map(|e| e.id), Some(12));
    assert!(w.get(12).is_none());
}

#[test]
fn a_rank_changed_while_waiting_decides_who_is_served() {
    // Thread 30 began waiting first at the higher rank; thread 31 has been
    // raised above it since.
    let mut w = WaitList::new();
    w.add(30, None).unwrap();
    w.add(31, None).unwrap();
    assert_eq!(w.next_waiting(&ranks(&[(30, 50), (31, 1)])).map(|e| e.id), Some(30));
    assert_eq!(w.next_waiting(&ranks(&[(30, 50), (31, 60)])).map(|e| e.id), Some(31));
}

#[test]
fn a_reserved_place_takes_a_message_past_maxmsg_without_allocating() {
    let mut q = MessageStore::new(1);
    q.reserve().unwrap();
    q.reserve().unwrap();
    q.push(0, b"sent".to_vec()).unwrap();
    assert!(q.is_full());
    assert_eq!(q.reserved(), 2);
    let expected_len = q.len() + 2;
    q.push_reserved(0, b"cookie-1".to_vec());
    q.push_reserved(0, b"cookie-2".to_vec());
    assert_eq!((q.len(), q.reserved()), (expected_len, 0));
    // A sent message still finds the queue full.
    assert_eq!(q.push(0, b"more".to_vec()), Err(Refused::Full(b"more".to_vec())));
    assert_eq!(q.bytes(), 4 + 8 + 8);
    assert_eq!(q.pop(), Some((b"sent".to_vec(), 0)));
    assert_eq!(q.pop(), Some((b"cookie-1".to_vec(), 0)));
}

#[test]
fn a_place_given_back_is_not_kept() {
    let mut q = MessageStore::new(4);
    q.reserve().unwrap();
    q.unreserve();
    assert_eq!(q.reserved(), 0);
    q.unreserve();
    assert_eq!(q.reserved(), 0);
}

#[test]
fn a_delivery_left_queued_goes_to_the_best_waiting_receiver() {
    let mut c: Channel<()> = Channel::new(1);
    c.receivers.add(40, None).unwrap();
    c.receivers.add(41, None).unwrap();
    c.msgs.reserve().unwrap();
    c.msgs.push_reserved(0, b"cookie".to_vec());
    assert_eq!(c.serve(&ranks(&[(41, 9)])), (true, false));
    assert_eq!(c.msgs.len(), 0);
    assert_eq!(c.receivers.get(41).and_then(|w| w.msg.clone()), Some((b"cookie".to_vec(), 0)));
    assert!(c.receivers.get(40).is_some_and(|w| !w.done));
    assert_eq!(c.serve(&ranks(UNRANKED)), (false, false));
}

#[test]
fn a_send_behind_a_queued_delivery_keeps_the_order() {
    // A delivery sits queued while a receiver waits; a send then queues
    // behind it, and the receiver is served the older message.
    let mut c: Channel<()> = Channel::new(4);
    c.receivers.add(50, None).unwrap();
    c.msgs.reserve().unwrap();
    c.msgs.push_reserved(0, b"older".to_vec());
    assert_eq!(c.send(0, b"newer".to_vec(), &ranks(UNRANKED)), Sent::Queued { notify: None, served: true });
    assert_eq!(c.receivers.get(50).and_then(|w| w.msg.clone()), Some((b"older".to_vec(), 0)));
    assert_eq!(c.receive(&ranks(UNRANKED)), Some(((b"newer".to_vec(), 0), false)));
}

#[test]
fn the_name_table_lists_in_name_order() {
    let mut table: NameTable<u32> = NameTable::new();
    for (name, v) in [(&b"b"[..], 2), (b"a", 1), (b"c", 3)] {
        table.insert(name, v).unwrap();
    }
    let listed: Vec<_> = table.iter().map(|(n, v)| (n.to_vec(), *v)).collect();
    assert_eq!(listed, [(b"a".to_vec(), 1), (b"b".to_vec(), 2), (b"c".to_vec(), 3)]);
}

#[test]
fn a_send_with_a_receiver_waiting_hands_the_message_over() {
    // Capacity 1 and one blocked receiver: the first send goes to the
    // receiver, so a second, nonblocking send still finds room.
    let mut c: Channel<&str> = Channel::new(1);
    c.notify = Some("registration");
    c.receivers.add(7, None).unwrap();
    assert_eq!(c.send(3, b"first".to_vec(), &ranks(UNRANKED)), Sent::HandedOff);
    assert_eq!(c.msgs.len(), 0);
    assert_eq!(c.receivers.get(7).and_then(|w| w.msg.clone()), Some((b"first".to_vec(), 3)));
    // The handed-off message used no registration; the second arrives at an
    // empty queue no receiver waits on, and uses it.
    assert_eq!(
        c.send(0, b"second".to_vec(), &ranks(UNRANKED)),
        Sent::Queued { notify: Some("registration"), served: false }
    );
    assert_eq!(c.msgs.len(), 1);
    assert_eq!(c.send(0, b"third".to_vec(), &ranks(UNRANKED)), Sent::Full(b"third".to_vec()));
}

#[test]
fn a_message_into_a_nonempty_queue_uses_no_registration() {
    let mut c: Channel<u8> = Channel::new(4);
    assert_eq!(c.send(0, b"a".to_vec(), &ranks(UNRANKED)), Sent::Queued { notify: None, served: false });
    c.notify = Some(1);
    assert_eq!(c.send(0, b"b".to_vec(), &ranks(UNRANKED)), Sent::Queued { notify: None, served: false });
    assert_eq!(c.notify, Some(1));
}

#[test]
fn a_receive_from_a_full_queue_queues_the_next_senders_message() {
    let mut c: Channel<()> = Channel::new(1);
    assert_eq!(c.send(0, b"full".to_vec(), &ranks(UNRANKED)), Sent::Queued { notify: None, served: false });
    c.senders.add(20, Some((b"low".to_vec(), 9))).unwrap();
    c.senders.add(21, Some((b"high".to_vec(), 1))).unwrap();
    let rank = ranks(&[(20, 1), (21, 60)]);
    let (taken, served) = c.receive(&rank).unwrap();
    assert_eq!(taken, (b"full".to_vec(), 0));
    assert!(served);
    // The higher-ranked sender got the room, though its message's priority
    // is lower.
    assert!(c.senders.get(21).is_some_and(|w| w.done));
    assert!(c.senders.get(20).is_some_and(|w| !w.done));
    assert_eq!(c.receive(&rank), Some(((b"high".to_vec(), 1), true)));
    assert_eq!(c.receive(&rank), Some(((b"low".to_vec(), 9), false)));
    assert_eq!(c.receive(&rank), None);
}

#[test]
fn a_queue_is_charged_its_messages_and_their_bookkeeping() {
    assert_eq!(queue_bytes(10, 8192), 10 * 8192 + 10 * 48 + 10 * 48);
    assert_eq!(queue_bytes(65536, 1), 65536 + 65536 * 48 + 32768 * 48);
    // The default limit holds nine queues of the default size.
    let default = queue_bytes(10, 8192);
    assert_eq!(DEFAULT_MSGQUEUE_BYTES / default, 9);
}

#[test]
fn the_charge_is_per_real_user_and_released_with_the_queue() {
    let mut u = Usage::new();
    let limit = 1000;
    assert_eq!(u.charge(5, 600, limit, false), Ok(()));
    assert_eq!(u.charge(5, 401, limit, false), Err(EMFILE));
    // Privilege does not lift RLIMIT_MSGQUEUE.
    assert_eq!(u.charge(5, 401, limit, true), Err(EMFILE));
    assert_eq!(u.charge(5, 400, limit, false), Ok(()));
    assert_eq!(u.charge(6, 1000, limit, false), Ok(()));
    assert_eq!(u.charged(5), 1000);
    assert_eq!(u.queues(), 3);
    u.release(5, 600);
    assert_eq!(u.charged(5), 400);
    assert_eq!(u.charge(5, u64::MAX, u64::MAX, false), Err(EMFILE));
    u.release(5, 400);
    u.release(6, 1000);
    assert_eq!((u.charged(5), u.charged(6), u.queues()), (0, 0, 0));
}

#[test]
fn queues_max_bounds_callers_without_privilege() {
    let mut u = Usage::new();
    for _ in 0..QUEUES_MAX {
        u.charge(1, 1, u64::MAX, false).unwrap();
    }
    assert_eq!(u.charge(2, 1, u64::MAX, false), Err(ENOSPC));
    assert_eq!(u.charge(0, 1, u64::MAX, true), Ok(()));
    u.release(0, 1);
    u.release(1, 1);
    assert_eq!(u.charge(2, 1, u64::MAX, false), Ok(()));
}

#[test]
fn the_name_table_finds_byte_names() {
    let mut t: NameTable<u32> = NameTable::new();
    for (i, name) in [b"b".as_slice(), b"\xff", b"a", b"ab"].into_iter().enumerate() {
        t.insert(name, i as u32).unwrap();
    }
    assert_eq!(t.get(b"\xff"), Some(&1));
    assert_eq!(t.get(b"a"), Some(&2));
    assert_eq!(t.get(b"ab"), Some(&3));
    assert_eq!(t.get(b"c"), None);
    assert_eq!(t.remove(b"a"), Some(2));
    assert_eq!(t.get(b"a"), None);
    assert_eq!(t.get(b"b"), Some(&0));
}
