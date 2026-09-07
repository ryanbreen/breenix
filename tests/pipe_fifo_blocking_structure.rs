//! Source ratchets for #813 PR-A. Structural evidence is not a boot result.
//! Mutations use owned source copies without changing the build working tree.
#[path = "blocking_fd_eagain_structure.rs"]
mod parser;
use parser::{compact, contains, function, functions, lex, match_arms, read, Node};
use std::collections::BTreeSet;

/// Follow lexical guard lifetimes, including explicit drop and nested extraction
/// scopes. This verifies the local ownership boundary, not scheduler semantics.
fn no_pm_at_cleanup(nodes: &[Node], inherited: &BTreeSet<String>) -> Result<usize, String> {
    let mut guards = inherited.clone();
    let mut edges = 0;
    let mut i = 0;
    while i < nodes.len() {
        if nodes[i].token() == "let" {
            if let Some(eq) = (i + 1..nodes.len())
                .take_while(|j| nodes[*j].token() != ";")
                .find(|j| nodes[*j].token() == "=")
            {
                let end = (eq + 1..nodes.len())
                    .find(|j| nodes[*j].token() == ";")
                    .unwrap_or(nodes.len());
                let expr = &nodes[eq + 1..end];
                // Only a direct manager initializer owns the guard in this scope;
                // a block initializer releases its locals on its closing brace.
                if !expr.iter().any(|n| n.group('{').is_some())
                    && (contains(expr, "manager()") || contains(expr, "PROCESS_MANAGER.lock()"))
                {
                    let binding = nodes[i + 1..eq]
                        .iter()
                        .find(|n| !["mut", ""].contains(&n.token()))
                        .ok_or("unrecognized PM binding")?
                        .token();
                    guards.insert(binding.into());
                }
            }
        }
        if nodes[i].token() == "drop" {
            if let Some(args) = nodes.get(i + 1).and_then(|n| n.group('(')) {
                if args.len() == 1 {
                    guards.remove(args[0].token());
                }
                if contains(args, "evicted") && !guards.is_empty() {
                    return Err("process row dropped under PM".into());
                }
            }
        }
        if [
            "deliver",
            "close_read",
            "close_write",
            "close_extracted_fds",
            "wake_up",
            "wake_up_one",
            "wake_up_n",
        ]
        .contains(&nodes[i].token())
            && nodes.get(i + 1).and_then(|n| n.group('(')).is_some()
        {
            edges += 1;
            if !guards.is_empty() {
                return Err(format!(
                    "{} reached with PM guards {guards:?}",
                    nodes[i].token()
                ));
            }
        }
        if let Node::Group(_, body) = &nodes[i] {
            edges += no_pm_at_cleanup(body, &guards)?;
        }
        i += 1;
    }
    Ok(edges)
}

#[test]
fn close_delivery_releases_pm_before_cleanup() {
    let src = read("kernel/src/syscall/pipe.rs");
    let body = function(&src, "sys_close");
    assert!(no_pm_at_cleanup(&body, &BTreeSet::new()).expect("PM-free close cleanup") >= 4);
    let broken = src.replace("drop(manager_guard);", "");
    assert_ne!(broken, src, "mutation must delete a real release");
    assert!(no_pm_at_cleanup(&function(&broken, "sys_close"), &BTreeSet::new()).is_err());
}

#[test]
fn new_writer_close_notifications_are_owned_and_deferred() {
    let pipe = read("kernel/src/ipc/pipe.rs");
    let tokens = compact(&lex(&pipe));
    assert!(tokens.contains("#[must_use=LITERAL]pubstructCloseNotifications"));
    for close in ["close_read", "close_write"] {
        let body = function(&pipe, close);
        assert!(contains(&body, "CloseNotifications{"));
        assert!(!contains(&body, "wake_up("));
        assert!(!contains(&body, "wake_up_one("));
        assert!(!contains(&body, "with_scheduler("));
    }
    let close = compact(&function(&pipe, "close_read"));
    assert!(close.contains("self.readers==1") && close.contains("self.readers-=1"));
    assert!(close.contains("writers:last_reader.then(||self.write_waiters.clone())"));
    assert!(contains(&function(&pipe, "deliver"), "queue.wake_up()"));
    let fifo = read("kernel/src/ipc/fifo.rs");
    for close in ["remove_reader", "remove_writer"] {
        let body = function(&fifo, close);
        assert!(
            !contains(&body, "buffer.lock()"),
            "registry must not double-decrement buffer"
        );
    }
    assert!(contains(
        &function(&fifo, "get_or_create_buffer"),
        "PipeBuffer::new_zero_refs()"
    ));
    let zero = compact(&function(&pipe, "new_zero_refs"));
    assert!(zero.contains("readers:0") && zero.contains("writers:0"));
    let anonymous = compact(&function(&pipe, "new"));
    assert!(anonymous.contains("pipe.readers=1") && anonymous.contains("pipe.writers=1"));
}

fn writer_edges(pipe: &str, helper: &str) -> Result<(), &'static str> {
    let tokens = compact(&lex(pipe));
    if !tokens.contains("write_waiters:Arc<WaitQueueHead>")
        || !tokens.contains("write_waiters:Arc::new(WaitQueueHead::new())")
    {
        return Err("writer queue");
    }
    let read = compact(&function(pipe, "read"));
    if !read.contains("ifread>0{self.write_waiters.wake_up();}") {
        return Err("drain wake");
    }
    let close = compact(&function(pipe, "close_read"));
    if !close.contains("writers:last_reader.then(||self.write_waiters.clone())")
        || !contains(&function(pipe, "deliver"), "queue.wake_up()")
    {
        return Err("close wake");
    }
    let write = compact(&function(pipe, "try_write"));
    if !write.contains("self.buffer[self.write_pos]=buf[written]")
        || !write.contains("self.len+=written")
        || !write.contains("Ok(written)")
    {
        return Err("copy/count");
    }
    if !write.contains("ifatomic_request{buf.len()}else{1}")
        || !write.contains("!self.has_write_space(required)")
    {
        return Err("atomic readiness");
    }
    let adapter = compact(&function(helper, "write_pipe"));
    if !adapter.contains("atomic_request=data.len()<=PIPE_BUF") {
        return Err("atomic threshold");
    }
    if !adapter.contains("pipe.try_write(&data[offset..],atomic_request)")
        || !adapter.contains("offset+=written")
    {
        return Err("progress/copy");
    }
    Ok(())
}

#[test]
fn writer_queue_notifications_atomic_copy_and_progress_exist() {
    writer_edges(
        &read("kernel/src/ipc/pipe.rs"),
        &read("kernel/src/syscall/blocking_io.rs"),
    )
    .unwrap();
}

#[test]
fn queue_wake_copy_and_threshold_mutations_are_detected() {
    let pipe = read("kernel/src/ipc/pipe.rs");
    let helper = read("kernel/src/syscall/blocking_io.rs");
    for (from, to, expected) in [
        ("pub write_waiters: Arc<WaitQueueHead>,", "", "writer queue"),
        (
            "write_waiters: Arc::new(WaitQueueHead::new()),",
            "",
            "writer queue",
        ),
        ("self.write_waiters.wake_up();", "", "drain wake"),
        (
            "writers: last_reader.then(|| self.write_waiters.clone()),",
            "writers: None,",
            "close wake",
        ),
        ("queue.wake_up();", "", "close wake"),
        (
            "self.buffer[self.write_pos] = buf[written];",
            "",
            "copy/count",
        ),
        ("Ok(written)", "Ok(buf.len())", "copy/count"),
    ] {
        let mutated = pipe.replace(from, to);
        assert_ne!(mutated, pipe, "mutation not applied: {from}");
        assert_eq!(writer_edges(&mutated, &helper), Err(expected));
    }
    let weak = helper.replace("data.len() <= PIPE_BUF", "data.len() < PIPE_BUF");
    assert_ne!(weak, helper);
    assert_eq!(writer_edges(&pipe, &weak), Err("atomic threshold"));
    let fabricated = helper.replace(
        "pipe.try_write(&data[offset..], atomic_request)",
        "Ok(data.len())",
    );
    assert_ne!(fabricated, helper);
    assert_eq!(writer_edges(&pipe, &fabricated), Err("progress/copy"));
}

#[test]
fn poll_and_transfer_share_one_readiness_predicate() {
    let pipe = read("kernel/src/ipc/pipe.rs");
    assert_eq!(
        compact(&function(&pipe, "has_write_space")),
        "self.readers>0&&(PIPE_BUF_SIZE-self.len)>=required"
    );
    assert!(contains(
        &function(&pipe, "try_write"),
        "has_write_space(required)"
    ));
    let arms = match_arms(&lex(&read("kernel/src/ipc/poll.rs")), "FdKind");
    for family in ["PipeWrite", "FifoWrite"] {
        let found: Vec<_> = arms
            .iter()
            .filter(|(names, _)| names.iter().any(|name| name == family))
            .collect();
        assert_eq!(found.len(), 1, "readiness census lost {family}");
        assert!(contains(&found[0].1, "has_write_space(1)"));
        assert!(contains(&found[0].1, "!pipe.has_readers()"));
        assert!(contains(&found[0].1, "POLLERR"));
        assert!(!contains(&found[0].1, "space()"));
    }
}

#[test]
fn prepared_sleep_owns_no_guard_and_cleans_up_eintr() {
    let source = read("kernel/src/syscall/blocking_io.rs");
    let defs = functions(&lex(&source));
    let sleep: Vec<_> = defs
        .iter()
        .filter(|(_, f)| contains(&f.body, "arch_halt_with_interrupts()"))
        .collect();
    assert_eq!(sleep.len(), 1, "one shared sleep lifecycle");
    let body = compact(&sleep[0].1.body);
    assert!(!body.contains(".lock()") && !body.contains("manager()"));
    assert_eq!(
        sleep[0].1.params.len(),
        2,
        "only queue and publication result may enter sleep"
    );
    assert_eq!(body.matches("preempt_enable()").count(), 1);
    assert_eq!(body.matches("preempt_disable()").count(), 1);
    assert!(
        body.find("check_signals_for_eintr()").unwrap() < body.find("with_scheduler(").unwrap()
    );
    assert!(body.contains("ThreadState::BlockedOnIO"));
    assert!(body.find("preempt_disable()").unwrap() < body.find("take_waiter(").unwrap());
    assert!(body.rfind("finish_wait()").unwrap() < body.rfind("errno::EINTR").unwrap());
    assert!(body.contains("PrepareOutcome::Mismatch=>{queue.finish_wait();returnOk(());}"));
    assert!(body
        .contains("PrepareOutcome::PublishFailed=>{queue.finish_wait();returnErr(errno::ESRCH);}"));
    assert!(!body.contains("target_arch") && !body.contains("timeout"));
    let adapter = &defs["write_pipe"].body;
    // Publication must be inside a lexical object scope which ends before the
    // sleep call, with no guard reacquired in the outer retry loop.
    fn check_scope(nodes: &[Node], sleeping: &str, held: bool, count: &mut usize) {
        let mut local = held;
        for (i, n) in nodes.iter().enumerate() {
            if n.token() == "let" {
                let end = (i..nodes.len())
                    .find(|j| nodes[*j].token() == ";")
                    .unwrap_or(nodes.len());
                let statement = &nodes[i..end];
                if !statement.iter().any(|n| n.group('{').is_some())
                    && contains(statement, ".lock()")
                {
                    local = true;
                }
            }
            if n.token() == sleeping && nodes.get(i + 1).and_then(|n| n.group('(')).is_some() {
                assert!(!local, "buffer lock crosses prepared sleep");
                *count += 1;
            }
            if let Node::Group(_, inner) = n {
                check_scope(inner, sleeping, local, count);
            }
        }
    }
    let mut count = 0;
    check_scope(adapter, sleep[0].0, false, &mut count);
    assert_eq!(count, 1);
    assert!(contains(adapter, "prepare_to_wait_checked(ThreadState::BlockedOnIO,None,||{pipe.has_readers()&&!pipe.has_write_space(required)})"));
}

fn all_function_bodies(nodes: &[Node], out: &mut Vec<(String, Vec<Node>)>) {
    for (i, node) in nodes.iter().enumerate() {
        if node.token() == "fn" {
            let name = nodes[i + 1].token();
            if let Some(body) = nodes[i + 2..]
                .iter()
                .take_while(|n| n.token() != ";")
                .find_map(|n| n.group('{'))
            {
                out.push((name.into(), body.to_vec()));
            }
        }
        if let Node::Group(_, body) = node {
            all_function_bodies(body, out);
        }
    }
}

#[test]
fn dup_exec_and_row_drop_keep_cleanup_outside_pm() {
    let handlers = read("kernel/src/syscall/handlers.rs");
    let dup = function(&handlers, "sys_dup2");
    assert!(no_pm_at_cleanup(&dup, &BTreeSet::new()).unwrap() > 0);
    let fd = read("kernel/src/ipc/fd.rs");
    for extract in ["dup2", "close_cloexec"] {
        let body = function(&fd, extract);
        for forbidden in [
            "close_read(",
            "close_write(",
            "deliver(",
            "close_fifo_read(",
            "close_fifo_write(",
        ] {
            assert!(
                !contains(&body, forbidden),
                "{extract} must extract, not clean up"
            );
        }
    }
    assert!(contains(
        &function(&fd, "close_cloexec"),
        "closes.entries.push("
    ));
    let mut callers = Vec::new();
    all_function_bodies(&lex(&read("kernel/src/syscall/handlers.rs")), &mut callers);
    let execs: Vec<_> = callers
        .iter()
        .filter(|(_, body)| contains(body, "DeferredFdCloses::default()"))
        .collect();
    assert_eq!(execs.len(), 3, "exec cleanup owner census changed");
    for (name, body) in execs {
        let body = compact(body);
        assert!(
            body.find("DeferredFdCloses::default()").unwrap() < body.find("manager()").unwrap(),
            "{name}: RAII cleanup must be declared before PM for reverse destruction"
        );
        assert!(body.contains("&mutcloses"));
    }
    let mut manager_functions = Vec::new();
    all_function_bodies(
        &lex(&read("kernel/src/process/manager.rs")),
        &mut manager_functions,
    );
    let execs: Vec<_> = manager_functions
        .iter()
        .filter(|(_, body)| contains(body, ".close_cloexec("))
        .collect();
    assert_eq!(
        execs.len(),
        4,
        "both architectures' exec/execv paths must be inventoried"
    );
    for (name, body) in execs {
        assert!(
            contains(body, "close_cloexec(closes)"),
            "{name} lost caller-owned cleanup"
        );
        assert!(
            !contains(body, "DeferredFdCloses::default()"),
            "{name} owns cleanup under caller PM"
        );
    }
    for path in [
        "kernel/src/task/process_task.rs",
        "kernel/src/syscall/handlers.rs",
    ] {
        let mut funcs = Vec::new();
        all_function_bodies(&lex(&read(path)), &mut funcs);
        let mut rows = 0;
        for (name, body) in funcs.iter().filter(|(_, b)| contains(b, "drop(evicted)")) {
            no_pm_at_cleanup(body, &BTreeSet::new())
                .unwrap_or_else(|e| panic!("{path}:{name}: {e}"));
            rows += 1;
        }
        assert!(rows > 0, "row-drop census went blind in {path}");
    }
}

#[test]
fn close_call_census_requires_delivery_or_the_documented_fault_carveout() {
    fn files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    // P-2/#919: close_all_fds() runs under caller-owned PM (Process::terminate()'s
    // 4 identified callers hold it -- see Process::close_all_fds()'s own
    // comment), so it cannot use the ordinary deliver() path -- that risks
    // acquiring SCHEDULER (Level 1) while PM (Level 2) is held. The carveout
    // now splits by which half of a pipe/FIFO end closed:
    //   - close_write(): its single return expression is the field literal
    //     `CloseNotifications { writers: None }`, not a computed decision --
    //     see close_write()'s own source, checked directly by
    //     new_writer_close_notifications_are_owned_and_deferred's close_write
    //     inline-wake-free assertion above -- so discarding it is a
    //     documented no-op, not a lost wake. Stays a fault discard.
    //     claim-lint:ok: kernel/src/ipc/pipe.rs's close_write() function body,
    //     read directly -- one return statement, the field literal shown.
    //   - close_read(): the 1-to-0 transition IS a real new-writer-close
    //     notification (P-2's defect). It must now deliver, via the PM-safe
    //     deliver_deferred() path (lock-free ISR wake buffer, not the
    //     scheduler lock inline), not the ordinary deliver().
    fn scan(
        nodes: &[Node],
        carveout: bool,
        ordinary: &mut usize,
        discarded: &mut usize,
        deferred: &mut usize,
    ) {
        let statements: Vec<_> = nodes.split(|n| n.token() == ";").collect();
        for (index, stmt) in statements.iter().enumerate() {
            let direct_close = stmt.windows(3).find_map(|w| {
                if w[0].token() == "."
                    && ["close_read", "close_write"].contains(&w[1].token())
                    && w[2].group('(').is_some()
                {
                    Some(w[1].token())
                } else {
                    None
                }
            });
            let Some(kind) = direct_close else {
                continue;
            };
            if carveout && kind == "close_write" {
                assert!(
                    contains(stmt, "let_should_notify=buffer.lock().close_"),
                    "fault discard must be explicit"
                );
                *discarded += 1;
            } else if carveout && kind == "close_read" {
                assert_eq!(
                    stmt.first().map(Node::token),
                    Some("let"),
                    "PM-held read close result needs an owned local"
                );
                let binding = stmt[1].token();
                assert!(
                    !binding.starts_with('_'),
                    "close_read under PM cannot discard its notification (P-2/#919)"
                );
                assert!(
                    statements.get(index + 1).is_some_and(|s| contains(
                        s,
                        &format!("{binding}.deliver_deferred()")
                    )),
                    "notification must deliver_deferred() after close statement drops temporary guard"
                );
                assert!(
                    !contains(stmt, ".deliver_deferred()"),
                    "chained deliver_deferred would retain temporary buffer guard"
                );
                assert!(
                    !statements
                        .get(index + 1)
                        .is_some_and(|s| contains(s, &format!("{binding}.deliver()"))),
                    "PM-held close must use deliver_deferred(), not deliver() (lock order)"
                );
                *deferred += 1;
            } else {
                *ordinary += 1;
                if contains(stmt, ".lock()") {
                    assert_eq!(
                        stmt.first().map(Node::token),
                        Some("let"),
                        "buffer close result needs an owned local"
                    );
                    let binding = stmt[1].token();
                    assert!(
                        !binding.starts_with('_'),
                        "ordinary close cannot discard notification"
                    );
                    assert!(
                        statements
                            .get(index + 1)
                            .is_some_and(|s| contains(s, &format!("{binding}.deliver()"))),
                        "notification must deliver after close statement drops temporary guard"
                    );
                    assert!(
                        !contains(stmt, ".deliver()"),
                        "chained deliver would retain temporary buffer guard"
                    );
                } else {
                    assert!(contains(stmt, ".deliver()"), "unguarded close must deliver");
                }
            }
        }
        for node in nodes {
            if let Node::Group(_, body) = node {
                scan(body, carveout, ordinary, discarded, deferred);
            }
        }
    }
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    files(&root.join("kernel/src"), &mut paths);
    let mut ordinary = 0;
    let mut discarded = 0;
    let mut deferred = 0;
    for path in paths {
        let source = std::fs::read_to_string(&path).unwrap();
        let is_process = path.ends_with("process/process.rs");
        let mut funcs = Vec::new();
        all_function_bodies(&lex(&source), &mut funcs);
        for (name, body) in funcs {
            let exception = is_process && name == "close_all_fds";
            scan(
                &body,
                exception,
                &mut ordinary,
                &mut discarded,
                &mut deferred,
            );
        }
        if is_process {
            assert_eq!(source.matches("#919/P-2").count(), 8);
        }
    }
    assert!(ordinary >= 12, "ordinary close census went blind");
    assert_eq!(
        discarded, 4,
        "close_write carveout restricted to two inline arms on each architecture"
    );
    assert_eq!(
        deferred, 4,
        "close_read PM-safe delivery restricted to two inline arms on each architecture"
    );
}

/// P-2/#919's fix depends on one safety property: the deferred delivery path
/// close_all_fds() now uses is not supposed to acquire the scheduler lock
/// inline, because close_all_fds() runs under caller-owned PROCESS_MANAGER
/// and scheduler.rs's "Lock Ordering Discipline" note forbids acquiring
/// SCHEDULER (Level 1) while holding PROCESS_MANAGER (Level 2). This checks
/// the mechanism itself, independent of the census above, and the three
/// mutations below (`deferred_delivery_never_touches_the_scheduler_lock`)
/// redden it: wake_up_deferred()'s wake call is the lock-free
/// isr_unblock_for_io() buffer, and deliver_deferred() routes to it rather
/// than the ordinary, scheduler-lock-acquiring wake_up().
fn deferred_delivery_uses_lock_free_wake(waitqueue: &str, pipe: &str) -> Result<(), &'static str> {
    let body = compact(&function(waitqueue, "wake_up_deferred"));
    if !body.contains("isr_unblock_for_io(waiter.tid())") {
        return Err("missing lock-free wake");
    }
    if body.contains("wake_waitqueue_thread")
        || body.contains("with_scheduler")
        || body.contains("wake_waiter(")
    {
        return Err("touches the scheduler lock");
    }
    if !contains(
        &function(pipe, "deliver_deferred"),
        "queue.wake_up_deferred()",
    ) {
        return Err("deliver_deferred does not route to the deferred wake");
    }
    if contains(&function(pipe, "deliver_deferred"), "queue.wake_up()") {
        return Err("deliver_deferred falls back to the scheduler-lock wake");
    }
    Ok(())
}

#[test]
fn deferred_delivery_never_touches_the_scheduler_lock() {
    let waitqueue = read("kernel/src/task/waitqueue.rs");
    let pipe = read("kernel/src/ipc/pipe.rs");
    deferred_delivery_uses_lock_free_wake(&waitqueue, &pipe).unwrap();

    let mutated_pipe = pipe.replace("queue.wake_up_deferred();", "queue.wake_up();");
    assert_ne!(mutated_pipe, pipe, "mutation must change a real call site");
    assert_eq!(
        deferred_delivery_uses_lock_free_wake(&waitqueue, &mutated_pipe),
        Err("deliver_deferred does not route to the deferred wake")
    );

    // A partial regression that keeps the lock-free wake but ALSO reaches the
    // scheduler lock (e.g. a stray direct wake alongside the buffered one)
    // must be caught too, not just an outright replacement.
    let mutated_wq = waitqueue.replace(
        "crate::task::scheduler::isr_unblock_for_io(waiter.tid());",
        "crate::task::scheduler::isr_unblock_for_io(waiter.tid());\
                crate::task::scheduler::wake_waitqueue_thread(waiter.tid());",
    );
    assert_ne!(
        mutated_wq, waitqueue,
        "mutation must change a real call site"
    );
    assert_eq!(
        deferred_delivery_uses_lock_free_wake(&mutated_wq, &pipe),
        Err("touches the scheduler lock")
    );

    // Deleting the lock-free wake outright (leaving the loop body empty of any
    // wake call) must also be caught.
    let dropped_wq = waitqueue.replace(
        "crate::task::scheduler::isr_unblock_for_io(waiter.tid());",
        "",
    );
    assert_ne!(
        dropped_wq, waitqueue,
        "mutation must change a real call site"
    );
    assert_eq!(
        deferred_delivery_uses_lock_free_wake(&dropped_wq, &pipe),
        Err("missing lock-free wake")
    );
}

fn quoted_names(body: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut parts = body.split('"');
    while parts.next().is_some() {
        let Some(name) = parts.next() else {
            break;
        };
        assert!(
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "unrecognized arm literal {name}"
        );
        assert!(names.insert(name.into()), "duplicate arm {name}");
    }
    names
}
fn oracle_arms(source: &str) -> BTreeSet<String> {
    let decl = source
        .split("const ARMS:")
        .nth(1)
        .expect("driver declares arm array");
    let initializer = decl.split_once('=').expect("array initializer").1;
    let list = initializer
        .split_once('[')
        .unwrap()
        .1
        .split_once(']')
        .unwrap()
        .0;
    let names = quoted_names(list);
    assert!(names.len() >= 13, "matrix census incomplete");
    let driver = compact(&function(source, "main"));
    assert!(
        driver.contains("ARMS.iter().enumerate()"),
        "driver no longer enumerates its matrix"
    );
    assert!(
        driver.contains("bytes==expected_bytes(arm)"),
        "each verdict needs a byte tally"
    );
    assert!(
        driver.contains("arch,kind,arm,bytes,expected_bytes(arm)"),
        "arm census is disconnected from emitted verdict"
    );
    names
}
fn gate_arms(source: &str) -> BTreeSet<String> {
    let array = source
        .split("EXPECTED_ARMS=(")
        .nth(1)
        .expect("gate expected arm array")
        .split_once(')')
        .unwrap()
        .0;
    let words: Vec<_> = array
        .lines()
        .flat_map(|line| line.split('#').next().unwrap().split_whitespace())
        .collect();
    let set: BTreeSet<_> = words
        .iter()
        .map(|s| s.trim_matches(['\'', '"']).to_string())
        .collect();
    assert_eq!(set.len(), words.len(), "duplicate expected gate arm");
    set
}
#[test]
fn gate_scores_the_drivers_emitted_arm_set_and_mutations_fail() {
    let driver = read("userspace/programs/src/pipe_fifo_blocking_oracle.rs");
    let gate = read("docker/qemu/run-blocking-io-oracle-gate.sh");
    let arms = oracle_arms(&driver);
    assert_eq!(arms, gate_arms(&gate));
    // Independently derive the set with byte-count definitions from actual match
    // patterns, so a declared but undriven arm cannot silently enter the gate.
    let tally = driver
        .split("fn expected_bytes(")
        .nth(1)
        .unwrap()
        .split("\n}")
        .next()
        .unwrap();
    let patterns = tally
        .lines()
        .filter_map(|line| line.split_once("=>").map(|(pattern, _)| pattern))
        .filter(|line| line.contains('"'))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(arms, quoted_names(&patterns));
    let mutated = gate.replace("EXPECTED_ARMS=(", "EXPECTED_ARMS=( invented_unscored_arm ");
    assert_ne!(mutated, gate);
    assert_ne!(arms, gate_arms(&mutated));
    let one = arms.iter().next().unwrap();
    let mutated = driver.replacen(&format!("\"{one}\""), "\"invented_driver_arm\"", 1);
    assert_ne!(mutated, driver);
    assert_ne!(oracle_arms(&mutated), gate_arms(&gate));
    assert!(gate.contains("gate_structure_preflight"));
    assert!(gate.contains("trap report_gate_failure ERR"));
}

#[test]
fn every_removed_process_row_is_retained_past_the_callers_pm_guard() {
    fn paths(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                paths(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    fn check_removals(
        nodes: &[Node],
        mut guards: BTreeSet<String>,
        mut safe_owners: BTreeSet<String>,
        owner: Option<&str>,
    ) -> usize {
        let mut count = 0;
        for (i, node) in nodes.iter().enumerate() {
            if node.token() == "let" {
                let end = (i + 1..nodes.len())
                    .find(|j| nodes[*j].token() == ";")
                    .unwrap_or(nodes.len());
                if let Some(eq) = (i + 1..end).find(|j| nodes[*j].token() == "=") {
                    if let Some(binding) = nodes[i + 1..eq]
                        .iter()
                        .find(|n| !["mut", ""].contains(&n.token()))
                    {
                        safe_owners.remove(binding.token());
                        let expr = &nodes[eq + 1..end];
                        if !expr.iter().any(|n| n.group('{').is_some()) {
                            if contains(expr, "manager()") {
                                guards.insert(binding.token().into());
                            } else if guards.is_empty() {
                                safe_owners.insert(binding.token().into());
                            }
                        }
                    }
                }
            }
            if node.token() == "drop" {
                if let Some(args) = nodes.get(i + 1).and_then(|n| n.group('(')) {
                    if args.len() == 1 {
                        guards.remove(args[0].token());
                    }
                }
            }
            if node.token() == "remove_process"
                && nodes.get(i + 1).and_then(|n| n.group('(')).is_some()
            {
                let holder =
                    owner.expect("remove_process result must be retained for PM-free Drop");
                assert!(
                    safe_owners.contains(holder),
                    "row holder {holder} must be declared outside and before its PM scope"
                );
                count += 1;
            }
            if let Node::Group(_, inner) = node {
                let nested_owner =
                    if i >= 3 && nodes[i - 1].token() == "push" && nodes[i - 2].token() == "." {
                        Some(nodes[i - 3].token())
                    } else {
                        owner
                    };
                count += check_removals(inner, guards.clone(), safe_owners.clone(), nested_owner);
            }
        }
        count
    }
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    paths(&root.join("kernel/src"), &mut files);
    let mut retained = 0;
    let mut fresh_init = 0;
    for path in files {
        let mut bodies = Vec::new();
        all_function_bodies(&lex(&std::fs::read_to_string(&path).unwrap()), &mut bodies);
        for (name, body) in bodies
            .iter()
            .filter(|(_, body)| contains(body, ".remove_process("))
        {
            if path.ends_with("process/manager.rs") && name == "hold_init_publication" {
                assert!(contains(body, "drop(self.remove_process(provisional_pid))"));
                fresh_init += 1;
                continue;
            }
            let count = check_removals(body, BTreeSet::new(), BTreeSet::new(), None);
            assert!(count > 0, "{name}: removal census went blind");
            retained += count;
        }
    }
    assert!(retained >= 20, "ordinary row removal census went blind");
    assert_eq!(
        fresh_init, 1,
        "only fresh stdio-only init publication may drop inline"
    );
}

#[test]
fn observation_seam_is_boot_only_and_queries_do_not_wake() {
    fn gated(source: &str, item: &str) -> bool {
        let Some(at) = source.find(item) else {
            return false;
        };
        let prefix = &source[..at];
        let Some(attr) = prefix.rfind("#[cfg(") else {
            return false;
        };
        prefix[attr..].split_whitespace().collect::<String>() == "#[cfg(feature=\"boot_tests\")]"
    }
    let module = read("kernel/src/syscall/mod.rs");
    let ioctl = read("kernel/src/syscall/ioctl.rs");
    assert!(gated(&module, "pub mod blocking_io_oracle;"));
    assert!(gated(
        &ioctl,
        "if request == super::blocking_io_oracle::QUERY"
    ));
    assert!(!gated(
        &module.replace("#[cfg(feature = \"boot_tests\")]", ""),
        "pub mod blocking_io_oracle;"
    ));
    assert!(!gated(
        &ioctl.replace("#[cfg(feature = \"boot_tests\")]", ""),
        "if request == super::blocking_io_oracle::QUERY"
    ));
    let source = read("kernel/src/syscall/blocking_io_oracle.rs");
    let query = compact(&function(&source, "query"));
    for forbidden in [
        "wake_up(",
        "prepare_to_wait",
        "try_write(",
        "yield_current(",
        "arch_halt",
        "finish_wait(",
    ] {
        assert!(
            !query.contains(forbidden),
            "observation query changes victim state via {forbidden}"
        );
    }
    assert!(query.find("copy_from_user(").unwrap() < query.find("manager()").unwrap());
    assert!(query.contains("target_pid!=caller_pid&&target.parent!=Some(caller_pid)"));
    assert!(query.contains("Arc::ptr_eq(&buffer,other)"));
    assert!(query.contains("write_waiters.contains_waiter(witness.tid)"));
    assert!(
        query.contains("ThreadState::BlockedOnIO") && query.contains("thread.blocked_in_syscall")
    );
    assert!(query.rfind("copy_to_user(").unwrap() > query.find("with_thread_mut(").unwrap());
}

fn aggregate_writev(source: &str) -> bool {
    let body = compact(&function(source, "sys_writev"));
    body.contains("length<=crate::ipc::pipe::PIPE_BUFasu64")
        && body.contains(
            "crate::ipc::FdKind::PipeWrite(buffer)|crate::ipc::FdKind::FifoWrite(_,buffer)",
        )
        && body.contains("forvectorin&vectors")
        && body.contains("copy_from_user(addressas*constu8)")
        && body.contains(
            "return super::blocking_io::write_pipe(&buffer,&gathered,nonblocking);"
                .replace(' ', "")
                .as_str(),
        )
        && body.find("write_pipe(") < body.find("handlers::sys_write(")
}

#[test]
fn writev_aggregate_atomicity_rejects_per_iovec_mutation() {
    let source = read("kernel/src/syscall/iovec.rs");
    assert!(
        aggregate_writev(&source),
        "small pipe/FIFO writev must gather into one transfer"
    );
    let mutation = source.replace(
        "return super::blocking_io::write_pipe(&buffer, &gathered, nonblocking);",
        "for vector in &vectors { handlers::sys_write(fd, vector.iov_base, vector.iov_len); }",
    );
    assert_ne!(mutation, source);
    assert!(
        !aggregate_writev(&mutation),
        "per-iovec mutation must be rejected"
    );
}

fn retained_wait_identity(source: &str, waitqueue: &str) -> bool {
    let body = compact(&function(source, "wait_prepared"));
    let finish = compact(&function(waitqueue, "finish_wait_for"));
    body.matches("current_thread_id()").count() == 1
        && body.find("let tid=".replace(' ', "").as_str()) < body.find("preempt_enable()")
        && body.contains("queue.take_waiter(tid);queue.finish_wait_for(tid);")
        && finish.contains("self.remove_waiter(tid)")
        && finish.contains("with_thread_mut(tid,")
        && !finish.contains("current_thread_id")
}

#[test]
fn prepared_wait_retains_identity_for_both_cleanup_operations() {
    let source = read("kernel/src/syscall/blocking_io.rs");
    let queue = read("kernel/src/task/waitqueue.rs");
    assert!(retained_wait_identity(&source, &queue));
    let mutation = source.replace("queue.take_waiter(tid);", "if let Some(tid) = crate::task::scheduler::current_thread_id() { queue.take_waiter(tid); }");
    assert_ne!(mutation, source);
    assert!(!retained_wait_identity(&mutation, &queue));
    let mutation = source.replace("queue.finish_wait_for(tid);", "queue.finish_wait();");
    assert!(!retained_wait_identity(&mutation, &queue));
}
