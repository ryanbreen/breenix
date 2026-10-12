//! Notices (`kernel/src/log_notice.rs`): a posted line survives its owner
//! moving, is written before its owner is freed, carries its own newline, and
//! the bounded COM1 writer's fallback posts a line whole or not at all.

extern crate alloc;

#[path = "../kernel/src/log_notice.rs"]
mod log_notice;

/// What the kernel's writers do with the list: here, collect the lines.
mod serial {
    pub fn write_notices() {
        crate::log_notice::take(|line| crate::WRITTEN.lock().unwrap().push(line.to_vec()));
    }
}

use log_notice::{Node, Notice};
use std::sync::Mutex;

static WRITTEN: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

/// The list is global: one test at a time.
static SERIAL: Mutex<()> = Mutex::new(());

const UDP: &str = "UDP: Delivered packet to socket on port ";

fn written() -> Vec<String> {
    serial::write_notices();
    WRITTEN
        .lock()
        .unwrap()
        .drain(..)
        .map(|line| String::from_utf8(line).unwrap())
        .collect()
}

#[test]
fn a_notice_that_moves_while_posted_is_written_whole() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    written();
    let mut sockets = vec![Notice::new()];
    sockets[0].post(UDP, 54321);
    // Moving the notices (the Vec grows, then a swap) must not move the posted line.
    for _ in 0..64 {
        sockets.push(Notice::new());
    }
    sockets.swap(0, 63);
    sockets[63].post(UDP, 1);
    assert_eq!(written(), vec![format!("{UDP}54321\n")]);
    assert!(!log_notice::pending());
}

#[test]
fn dropping_a_posted_notice_writes_its_line_first() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    written();
    let notice = Notice::new();
    notice.post(UDP, 7);
    let moved = Box::new(notice);
    drop(moved);
    assert!(!log_notice::pending());
    assert_eq!(written(), vec![format!("{UDP}7\n")]);
}

#[test]
fn a_long_line_is_cut_and_keeps_its_newline() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    written();
    let notice = Notice::new();
    let long = "x".repeat(200);
    notice.post(&long, 12345);
    let lines = written();
    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    assert!(line.ends_with("12345\n"), "{line:?}");
    assert_eq!(line.matches('\n').count(), 1);
}

#[test]
fn a_busy_port_posts_lines_whole_and_keeps_room_for_stage_lines() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    written();
    static SPARE: [Node; 4] = [const { Node::new() }; 4];
    let busy = || false;

    // Ordinary lines may take only the first half.
    log_notice::write_or_post(&SPARE, "ordinary 1", false, 3, busy);
    log_notice::write_or_post(&SPARE, "ordinary 2", false, 3, busy);
    log_notice::write_or_post(&SPARE, "ordinary 3", false, 3, busy);
    // Stage lines take what is left.
    log_notice::write_or_post(&SPARE, "RING3_SYSCALL a", true, 3, busy);
    log_notice::write_or_post(&SPARE, "RING3_SYSCALL b", true, 3, busy);

    // Every node is now waiting: a stage line keeps trying the port.
    let mut tries = 0;
    log_notice::write_or_post(&SPARE, "RING3_SYSCALL c", true, 3, || {
        tries += 1;
        tries == 50
    });
    assert_eq!(tries, 50, "the stage line stopped trying the port");

    assert_eq!(
        written(),
        vec![
            "ordinary 1\n",
            "ordinary 2\n",
            "RING3_SYSCALL a\n",
            "RING3_SYSCALL b\n"
        ]
    );

    // Written out, the nodes are free again; a free port takes the line itself.
    log_notice::write_or_post(&SPARE, "ordinary 4", false, 3, busy);
    let mut wrote = false;
    log_notice::write_or_post(&SPARE, "ordinary 5", false, 3, || {
        wrote = true;
        true
    });
    assert!(wrote);
    assert_eq!(written(), vec!["ordinary 4\n"]);
}
