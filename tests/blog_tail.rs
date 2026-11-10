#[path = "../userspace/programs/src/blog/tail.rs"]
mod tail;

use tail::{Action, Tail, MAX_FILE_SIZE};

#[test]
fn partial_lines_wait_for_newline() {
    let mut tail = Tail::new();
    tail.feed(b"one\ntw");
    assert_eq!(tail.line_count(), 1);
    assert_eq!(tail.line(0), b"one");
    tail.feed(b"o\nthree\n");
    assert_eq!((tail.line(1), tail.line(2)), (&b"two"[..], &b"three"[..]));
}

#[test]
fn truncation_reloads_from_zero() {
    let mut tail = Tail::new();
    assert_eq!(tail.observe(7, 8), Action::Read);
    tail.feed(b"old line\n");
    assert_eq!(tail.offset(), 9);
    assert_eq!(tail.observe(7, 4), Action::Reload);
    tail.feed(b"new\n");
    assert_eq!(tail.line_count(), 1);
    assert_eq!(tail.line(0), b"new");
    assert_eq!(tail.line_number(0), 1);
}

#[test]
fn renamed_file_reopens_from_zero() {
    let mut tail = Tail::new();
    tail.observe(7, 4);
    tail.feed(b"old\n");
    assert_eq!(tail.observe(8, 8), Action::Reopen);
    tail.feed(b"new file\n");
    assert_eq!(tail.line_count(), 1);
    assert_eq!(tail.line(0), b"new file");
}

#[test]
fn cap_discards_whole_lines_and_preserves_absolute_numbers() {
    let mut tail = Tail::new();
    tail.feed(b"first\n");
    let long = vec![b'x'; MAX_FILE_SIZE - 6];
    tail.feed(&long);
    tail.feed(b"\nlast\n");
    assert_eq!(tail.line_count(), 2);
    assert_eq!(tail.line_number(0), 2);
    assert_eq!(tail.line(0), long);
    assert_eq!(tail.line_number(1), 3);
    assert_eq!(tail.line(1), b"last");
}
