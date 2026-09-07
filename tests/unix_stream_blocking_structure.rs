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
        && matches!((body.find("drop(rx_buf)"), body.find("writers.wake_up()")), (Some(drop_at), Some(wake_at)) if drop_at < wake_at)
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
        unix.replace("drop(rx_buf);", ""),
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
fn transmit_predicate_ok(unix: &str) -> bool {
    let state_impl = unix.split("impl UnixWriteState<'_>").nth(1).unwrap_or("");
    compact(&function(state_impl, "has_write_space"))
        == "!*self.peer_closed&&self.buffer.len()<UNIX_SOCKET_BUFFER_SIZE"
        && compact(&function(
            unix.split("impl UnixWriteState").next().unwrap(),
            "has_write_space",
        )) == "self.writer().state().has_write_space()"
}

#[test]
fn transmit_predicate_and_single_mutations() {
    let unix = read("kernel/src/socket/unix.rs");
    assert!(transmit_predicate_ok(&unix));
    for changed in [
        unix.replace(
            "!*self.peer_closed && self.buffer.len() < UNIX_SOCKET_BUFFER_SIZE",
            "!*self.peer_closed",
        ),
        unix.replace(
            "self.buffer.len() < UNIX_SOCKET_BUFFER_SIZE",
            "self.buffer.len() <= UNIX_SOCKET_BUFFER_SIZE",
        ),
        unix.replace(
            "self.writer().state().has_write_space()",
            "!self.peer_closed()",
        ),
    ] {
        assert!(!transmit_predicate_ok(&changed));
    }
}

#[test]
fn guarded_state_defines_transmit_space_and_publication() {
    let unix = read("kernel/src/socket/unix.rs");
    let state = compact(&function(&unix, "state"));
    assert!(state.contains("UnixEndpoint::A=>(&self.pair.buffer_a_to_b,&self.pair.closed_b)"));
    assert!(state.contains("UnixEndpoint::B=>(&self.pair.buffer_b_to_a,&self.pair.closed_a)"));
    assert!(
        state.find("buffer.lock()").expect("buffer guard")
            < state.find("closed.lock()").expect("closed guard")
    );
    assert!(unix.contains("peer_closed: MutexGuard<'a, bool>"));
    assert!(unix.contains("buffer: MutexGuard<'a, VecDeque<u8>>"));
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
    assert!(
        body.find("process::fork()").expect("fork")
            < body.find("socket::socket(").expect("endpoint creation")
    );
    assert!(body.contains("wait_witness(peer,writer,tid)"));
    assert!(body.contains("reap(pid)"));
    assert!(gate.contains("gate_structure_preflight"));
    assert!(gate.contains("records.count(f'{arch}:{arm}:verdict=PASS:bytes={expected[arm]}') == 1"));
}

#[test]
fn close_notifications_leave_pm_without_the_isr_buffer() {
    let unix = read("kernel/src/socket/unix.rs");
    let close = compact(&function(&unix, "close"));
    assert!(close.contains("UnixCloseNotifications{pair:self.pair.clone(),}"));
    assert!(!close.contains("with_scheduler"));
    assert!(!close.contains("wake_up"));
    let deferred = compact(&function(&unix, "deliver_deferred"));
    assert!(deferred.contains("PENDING_CLOSES.lock()"));
    assert!(!deferred.contains("isr_unblock"));
    assert!(!deferred.contains("wake_up_deferred"));
    assert!(!deferred.contains("with_scheduler"));
    assert!(!deferred.contains("Vec::"));
    let drain = compact(&function(&unix, "drain_close_notifications"));
    assert!(drain.contains("head.take()"));
    assert!(drain.contains("next.take()"));
    assert!(drain.contains("queued=false"));
    assert!(drain.contains("UnixCloseNotifications{pair}.deliver()"));
    let process = compact(&lex(&read("kernel/src/process/process.rs")));
    assert_eq!(process.matches("FdKind::UnixStream(socket)=>{letnotifications=socket.lock().close();notifications.deliver_deferred();}").count(), 2);
    let drop_fd = compact(&lex(&read("kernel/src/ipc/fd.rs")));
    assert!(drop_fd
        .contains("letnotifications=socket.lock().close();notifications.deliver_deferred();"));
    let boundary = compact(&function(
        &read("kernel/src/task/process_task.rs"),
        "reclaim_deferred_process_resources",
    ));
    let drain_at = boundary
        .find("drain_close_notifications()")
        .expect("drain call");
    for guard in [
        "process_manager_held_on_current_cpu()",
        "scheduler_scope_active()",
    ] {
        assert!(boundary.find(guard).expect("context guard") < drain_at);
    }
    assert!(
        boundary
            .find("reclaim_preempt_disable()")
            .expect("preemption guard")
            < drain_at
    );
    assert!(boundary.find("compare_exchange(").expect("drain ownership") < drain_at);
    assert!(
        drain_at
            < boundary
                .find("reclaim_deferred_process_resources_for_pass(")
                .expect("reclaim pass")
    );
    for path in [
        "kernel/src/task/process_task.rs",
        "kernel/src/syscall/pipe.rs",
    ] {
        assert!(
            compact(&lex(&read(path)))
                .contains("letnotifications=socket.lock().close();notifications.deliver();"),
            "{path}"
        );
    }
}
