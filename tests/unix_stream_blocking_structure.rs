//! Bounded source ratchets for Unix direction, mode, and wait publication.
#[path = "blocking_fd_eagain_structure.rs"]
mod census;
use census::{compact, function, lex, match_arms, read};

fn mode_ok(handlers: &str) -> bool {
    match_arms(&lex(handlers), "FdKind").iter().any(|(names, body)| {
        names.iter().any(|n| n == "Socket")
            && compact(body).contains("is_nonblocking:(fd_entry.status_flags()&crate::ipc::fd::status_flags::O_NONBLOCK)!=0")
    })
}
fn drain_ok(unix: &str) -> bool {
    let body = compact(&function(unix, "read"));
    body.contains("UnixEndpoint::A=>&self.pair.write_waiters_b")
        && body.contains("UnixEndpoint::B=>&self.pair.write_waiters_a")
        && body.contains("writers.wake_up()")
        && matches!((body.find("drop(rx_buf)"), body.find("writers.wake_up()")), (Some(drop_at), Some(wake_at)) if drop_at < wake_at)
}
/// A connected Unix socket's readiness: UnixSock::poll's Connected arm.
fn poll_ok(unix: &str) -> bool {
    let body = function(unix, "poll");
    match_arms(&body, "UnixState").iter().any(|(names, arm)| {
        names.iter().any(|n| n == "Connected")
            && compact(arm).contains("if socket.has_write_space()".replace(' ', "").as_str())
            && compact(arm).contains("POLLHUP")
    })
}
#[test]
fn descriptor_mode_drain_and_poll_are_connected() {
    assert!(mode_ok(&read("kernel/src/syscall/handlers.rs")));
    assert!(drain_ok(&read("kernel/src/socket/unix.rs")));
    assert!(poll_ok(&read("kernel/src/socket/unix.rs")));
}
#[test]
fn mode_drain_and_poll_mutations_are_rejected_singly() {
    let handlers = read("kernel/src/syscall/handlers.rs");
    assert!(!mode_ok(
        &handlers.replace("fd_entry.status_flags()", "endpoint_flags()")
    ));
    let unix = read("kernel/src/socket/unix.rs");
    for changed in [
        unix.replace("writers.wake_up();", ""),
        unix.replace("drop(rx_buf);", ""),
        unix.replace("&self.pair.write_waiters_b", "&self.pair.write_waiters_a"),
    ] {
        assert!(!drain_ok(&changed));
    }
    assert!(mode_ok(&handlers));
    let unix = read("kernel/src/socket/unix.rs");
    assert!(poll_ok(&unix));
    for replacement in ["!socket.peer_closed()", "socket.has_data()"] {
        let changed = unix.replace("if socket.has_write_space()", &format!("if {replacement}"));
        assert_ne!(changed, unix, "poll mutation did not apply");
        assert!(!poll_ok(&changed));
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
    let helper = compact(&function(&unix, "write_unix"));
    assert!(helper.contains("queue.prepare_to_wait_checked(ThreadState::BlockedOnIO,None,||{!state.peer_closed()&&!state.has_write_space()})"));
    assert!(helper.contains("blocking_io::wait_prepared(&queue,outcome)"));
    assert!(!helper.contains("PIPE_BUF"));
    assert!(helper.contains("returnOk(count)"));
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
    let scorer = read("scripts/score-blocking-io-oracle.py");
    assert!(
        scorer.contains("records.count(f'{arch}:{arm}:verdict=PASS:bytes={expected[arm]}') == 1")
    );
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
    // A connected socket's endpoint closes in UnixSock::release, which runs
    // once, for the last descriptor: inline in thread context without PM, or
    // from the release queue that the reclamation boundary drains.
    let release = compact(&function(&unix, "release"));
    assert!(release.contains("letnotifications=stream.lock().close();notifications.deliver();"));
    let object = read("kernel/src/socket/object.rs");
    assert!(socket_release_leaves_pm(&object));
    for path in [
        "kernel/src/process/process.rs",
        "kernel/src/ipc/fd.rs",
        "kernel/src/task/process_task.rs",
        "kernel/src/syscall/pipe.rs",
    ] {
        assert!(
            !compact(&lex(&read(path))).contains(".close();notifications"),
            "{path} closes a Unix endpoint by hand again"
        );
    }
    let boundary = compact(&function(
        &read("kernel/src/task/process_task.rs"),
        "reclaim_deferred_process_resources",
    ));
    for drain in ["drain_close_notifications()", "drain_release_queue()"] {
        let drain_at = boundary.find(drain).expect("drain call");
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
    }
}

/// The last reference to a socket releases it inline only where a release
/// may run (no PM held, not in interrupt context, not in a scheduler scope);
/// otherwise it queues the socket, and the drain checks the same context.
fn socket_release_leaves_pm(object: &str) -> bool {
    let context = compact(&function(object, "release_may_run_here"));
    let drop_body = compact(&function(object, "drop"));
    let drain = compact(&function(object, "drain_release_queue"));
    context.contains("in_interrupt()")
        && context.contains("process_manager_held_on_current_cpu()")
        && context.contains("scheduler_scope_active()")
        && drop_body.contains(
            "ifrelease_may_run_here(){LIVE_SOCKETS.fetch_sub(1,Ordering::Relaxed);self.ops().release(self);return;}",
        )
        && drop_body.contains("queue.push(carcass)")
        && drain.contains("if!release_may_run_here(){return;}")
        && drain.contains("carcass.ops().release(&carcass)")
}

#[test]
fn socket_release_rule_rejects_an_unguarded_release() {
    let object = read("kernel/src/socket/object.rs");
    assert!(socket_release_leaves_pm(&object));
    for (from, to) in [
        ("|| crate::process::process_manager_held_on_current_cpu()", ""),
        ("if release_may_run_here() {", "if true {"),
        ("if !release_may_run_here() {\n        return;\n    }", ""),
    ] {
        let changed = object.replace(from, to);
        assert_ne!(changed, object, "release mutation did not apply: {from}");
        assert!(!socket_release_leaves_pm(&changed), "accepted {from} -> {to}");
    }
}

#[test]
fn unix_scorer_rejects_incomplete_or_corrupt_arm_sets() {
    use std::{fs, process::Command};
    let arms = [
        "backpressure",
        "mode",
        "poll",
        "peer_close",
        "signal",
        "partial",
    ];
    let counts = [139264, 139264, 131074, 0, 270338, 262144];
    let mut good = String::from("TSS IST[0] (double fault stack): 0x0\n");
    for (arm, count) in arms.iter().zip(counts) {
        good.push_str(&format!(
            "[UNIX_WRITE_ORACLE:x86_64:{arm}:verdict=PASS:bytes={count}]\n"
        ));
    }
    good.push_str(
        "[UNIX_WRITE_SUMMARY:x86_64:passed=6:failed=0]\n[UNIX_WRITE_RESULT:x86_64:status=0]\n",
    );
    let directory =
        std::env::temp_dir().join(format!("breenix-unix-scorer-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let run = |serial: &str| {
        fs::write(directory.join("serial.txt"), serial).unwrap();
        Command::new("python3")
            .arg(format!(
                "{}/scripts/score-blocking-io-oracle.py",
                env!("CARGO_MANIFEST_DIR")
            ))
            .arg("x86_64")
            .arg(&directory)
            .args(["--program", "unix_stream_blocking_oracle"])
            .args(arms)
            .output()
            .unwrap()
    };
    let result = run(&good);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let first = "[UNIX_WRITE_ORACLE:x86_64:backpressure:verdict=PASS:bytes=139264]\n";
    for bad in [
        good.replace(first, ""),
        good.replacen("bytes=139264", "bytes=139263", 1),
        format!("{good}{first}"),
        good.replace("status=0", "status=1"),
        good.replace("passed=6", "passed=5"),
        good.replacen("verdict=PASS", "verdict=FAIL", 1),
        format!("{good}TSS IST[0] (double fault stack): 0x0 DOUBLE FAULT\n"),
    ] {
        assert!(
            !run(&bad).status.success(),
            "accepted altered Unix evidence"
        );
    }
    fs::remove_dir_all(directory).unwrap();
}
