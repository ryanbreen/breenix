extern crate alloc;

#[path = "../kernel/src/fs/ufs/table.rs"]
mod table;

use table::{Error, RequestTable, QUEUE_CAP};

#[test]
fn matches_replies_and_sends_oldest_first() {
    let mut table = RequestTable::new();
    let a = table.submit("first", 10, 100).unwrap();
    let b = table.submit("second", 20, 100).unwrap();
    assert_ne!(a, b);
    assert_eq!(table.peek_next(), Some((a, &"first")));
    assert_eq!(table.take_next(), Some((a, "first")));
    assert_eq!(table.take_next(), Some((b, "second")));
    assert_eq!(table.take_next(), None);
    assert_eq!(table.complete(b, "reply b"), Some((20, Ok("reply b"))));
    assert_eq!(table.complete(a, "reply a"), Some((10, Ok("reply a"))));
    assert_eq!(table.complete(a, "duplicate"), None);
    assert_eq!(table.complete(u64::MAX, "unknown"), None);
}

#[test]
fn a_copy_racing_expiry_does_not_consume_the_next_record() {
    let mut table = RequestTable::<_, _, ()>::new();
    let old = table.submit("copying", 1, 10).unwrap();
    let live = table.submit("queued", 2, 20).unwrap();
    assert_eq!(table.peek_next(), Some((old, &"copying")));
    assert_eq!(table.expire(10), vec![(1, Err(Error::Eio))]);
    table.mark_sent(old);
    assert_eq!(table.peek_next(), Some((live, &"queued")));
    table.mark_sent(live);
    assert_eq!(table.peek_next(), None);
    table.cancel(live);
    assert_eq!(table.complete(live, ()), None);
}

#[test]
fn limits_in_flight_requests_until_completion() {
    let mut table = RequestTable::<usize, usize, ()>::new();
    assert!(table.has_capacity());
    let ids: Vec<_> = (0..QUEUE_CAP).map(|i| table.submit(i, i, 100).unwrap()).collect();
    assert!(!table.has_capacity());
    assert_eq!(table.submit(99, 99, 100), Err(Error::Full));
    assert_eq!(table.take_next(), Some((ids[0], 0)));
    assert!(!table.has_capacity());
    assert_eq!(table.submit(99, 99, 100), Err(Error::Full));
    assert_eq!(table.complete(ids[0], ()), Some((0, Ok(()))));
    assert!(table.has_capacity());
    assert!(table.submit(99, 99, 100).is_ok());
}

#[test]
fn expiry_fails_waiters_and_discards_late_replies() {
    let mut table = RequestTable::<_, _, &str>::new();
    let expired = table.submit("sent", 1, 10).unwrap();
    let unsent = table.submit("unsent", 2, 10).unwrap();
    let live = table.submit("live", 3, 11).unwrap();
    assert_eq!(table.take_next(), Some((expired, "sent")));
    assert!(table.expire(9).is_empty());
    assert_eq!(table.expire(10), vec![(1, Err(Error::Eio)), (2, Err(Error::Eio))]);
    assert_eq!(table.complete(expired, "late"), None);
    assert_eq!(table.take_next(), Some((live, "live")));
    assert_eq!(table.complete(live, "ok"), Some((3, Ok("ok"))));
    assert_eq!(table.complete(unsent, "late"), None);
}

#[test]
fn death_returns_pending_waiters_and_rejects_new_requests() {
    let mut table = RequestTable::<_, _, ()>::new();
    let sent = table.submit("sent", 1, 10).unwrap();
    table.submit("unsent", 2, 10).unwrap();
    assert_eq!(table.take_next(), Some((sent, "sent")));
    assert_eq!(table.dead(), vec![(1, Err(Error::Enotconn)), (2, Err(Error::Enotconn))]);
    assert_eq!(table.take_next(), None);
    assert_eq!(table.complete(sent, ()), None);
    assert!(table.dead().is_empty());
    assert_eq!(table.submit("new", 3, 10), Err(Error::Enotconn));
}
