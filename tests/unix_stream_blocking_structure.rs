//! Bounded source ratchets for Unix direction, mode, and wait publication.
#[path = "blocking_fd_eagain_structure.rs"]
mod census;
use census::{compact, function, lex, match_arms, read};

fn mode_ok(handlers: &str) -> bool {
    match_arms(&lex(handlers), "FdKind").iter().any(|(names, body)| {
        names.iter().any(|n| n == "UnixStream")
            && compact(body).contains("is_nonblocking:(fd_entry.status_flags&crate::ipc::fd::status_flags::O_NONBLOCK)!=0")
    })
}
fn drain_ok(unix: &str) -> bool {
    let body = compact(&function(unix, "read"));
    body.contains("UnixEndpoint::A=>&self.pair.write_waiters_b")
        && body.contains("UnixEndpoint::B=>&self.pair.write_waiters_a")
        && body.contains("writers.wake_up()")
        && body.find("drop(rx_buf)") < body.find("writers.wake_up()")
}
fn poll_ok(poll: &str) -> bool {
    match_arms(&lex(poll), "FdKind")
        .iter()
        .any(|(names, body)| {
            names.iter().any(|n| n == "UnixStream")
                && compact(body).contains("if socket.has_write_space()".replace(' ', "").as_str())
                && compact(body).contains("POLLHUP")
        })
}
#[test]
fn descriptor_mode_drain_and_poll_are_connected() {
    assert!(mode_ok(&read("kernel/src/syscall/handlers.rs")));
    assert!(drain_ok(&read("kernel/src/socket/unix.rs")));
    assert!(poll_ok(&read("kernel/src/ipc/poll.rs")));
}
#[test]
fn mode_drain_and_poll_mutations_are_rejected_singly() {
    let handlers = read("kernel/src/syscall/handlers.rs");
    assert!(!mode_ok(
        &handlers.replace("fd_entry.status_flags", "endpoint_flags")
    ));
    let unix = read("kernel/src/socket/unix.rs");
    for changed in [
        unix.replace("writers.wake_up();", ""),
        unix.replace("&self.pair.write_waiters_b", "&self.pair.write_waiters_a"),
    ] {
        assert!(!drain_ok(&changed));
    }
    let poll = read("kernel/src/ipc/poll.rs");
    for replacement in ["!socket.peer_closed()", "socket.has_data()"] {
        assert!(!poll_ok(
            &poll.replace("socket.has_write_space()", replacement)
        ));
    }
}
#[test]
fn guarded_state_defines_transmit_space_and_publication() {
    let unix = read("kernel/src/socket/unix.rs");
    let state = compact(&function(&unix, "state"));
    assert!(state.contains("UnixEndpoint::A=>(&self.pair.buffer_a_to_b,&self.pair.closed_b)"));
    assert!(state.contains("UnixEndpoint::B=>(&self.pair.buffer_b_to_a,&self.pair.closed_a)"));
    assert!(state.find("buffer.lock()") < state.find("closed.lock()"));
    assert!(compact(&function(&unix, "copy")).contains("self.has_write_space()"));
    let helper = compact(&function(
        &read("kernel/src/syscall/blocking_io.rs"),
        "write_unix",
    ));
    assert!(helper.contains("queue.prepare_to_wait_checked(ThreadState::BlockedOnIO,None,||{!state.peer_closed()&&!state.has_write_space()})"));
    assert!(helper.contains("wait_prepared(&queue,outcome)"));
    assert!(!helper.contains("PIPE_BUF"));
    assert!(helper.contains("returnSyscallResult::Ok(countasu64)"));
}
#[test]
fn driver_and_gate_have_six_scored_legs() {
    let driver = read("userspace/programs/src/unix_stream_blocking_oracle.rs");
    let gate = read("docker/qemu/run-blocking-io-oracle-gate.sh");
    for arm in [
        "backpressure",
        "mode",
        "poll",
        "peer_close",
        "signal",
        "partial",
    ] {
        assert!(driver.contains(&format!("\"{arm}\"")));
        assert!(gate.contains(arm));
    }
    let body = compact(&function(&driver, "connection"));
    assert!(body.find("process::fork()") < body.find("socket::socket("));
    assert!(body.contains("wait_witness(peer,writer,tid)"));
    assert!(body.contains("reap(pid)"));
    assert!(gate.contains("gate_structure_preflight"));
    assert!(gate.contains("records.count(f'{arch}:{arm}:verdict=PASS:bytes={expected[arm]}') == 1"));
}

#[test]
fn close_returns_owned_peer_notifications_and_pm_consumers_defer() {
    let unix = read("kernel/src/socket/unix.rs");
    let close = compact(&function(&unix, "close"));
    assert!(close.contains("UnixEndpoint::A=>self.pair.write_waiters_b.clone()"));
    assert!(close.contains("UnixEndpoint::B=>self.pair.write_waiters_a.clone()"));
    assert!(close.contains("UnixCloseNotifications{writers}"));
    assert!(!close.contains("writers.wake_up"));
    let process = compact(&lex(&read("kernel/src/process/process.rs")));
    assert_eq!(process.matches("FdKind::UnixStream(socket)=>{letnotifications=socket.lock().close();notifications.deliver_deferred();}").count(), 2);
    for path in [
        "kernel/src/ipc/fd.rs",
        "kernel/src/task/process_task.rs",
        "kernel/src/syscall/pipe.rs",
    ] {
        let text = compact(&lex(&read(path)));
        assert!(
            text.contains("letnotifications=socket.lock().close();notifications.deliver();"),
            "{path}"
        );
    }
}
