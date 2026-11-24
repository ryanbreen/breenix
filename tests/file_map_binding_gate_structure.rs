//! The per-file binding gate is taken inside exec and mmap system calls, which
//! run with preemption disabled on both architectures. `yield_current` there
//! only sets need_resched, so a retry loop around it keeps the CPU
//! (kernel/src/task/scheduler.rs yield_current). When an exec holding the gate
//! blocks on disk I/O and all of the CPUs are retrying, the woken holder gets
//! no CPU back (ARM 25.11d2 acceptance: 4 of 4 CPUs in
//! load_image's retry loop during utilities scripts/wordfreq-n). A waiter must
//! sleep on a wait queue that the gate's release wakes.
use std::{fs, path::PathBuf};

fn root() -> PathBuf {
    let source = PathBuf::from(file!());
    let source = if source.is_absolute() { source } else { std::env::current_dir().unwrap().join(source) };
    source.parent().unwrap().parent().unwrap().to_path_buf()
}

/// Bodies of the functions in `source` whose text contains `needle`, from
/// the `fn` keyword to the matching closing brace.
fn bodies_containing<'a>(source: &'a str, needle: &str) -> Vec<(&'a str, &'a str)> {
    let mut found = Vec::new();
    let mut search = 0;
    while let Some(offset) = source[search..].find("fn ") {
        let start = search + offset;
        search = start + 3;
        let name_end = source[search..].find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map_or(source.len(), |end| search + end);
        let Some(open) = source[start..].find('{').map(|at| start + at) else { break };
        if source[start..open].contains(';') {
            continue;
        }
        let mut depth = 0usize;
        let mut close = None;
        for (at, byte) in source[open..].bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + at + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let close = close.expect("unbalanced braces");
        let body = &source[start..close];
        if body.contains(needle) && !body[3..].contains("fn ") {
            found.push((&source[search..name_end], body));
        }
    }
    found
}

#[test]
fn binding_gate_waiters_sleep_until_release() {
    let file_map = fs::read_to_string(root().join("kernel/src/memory/file_map.rs")).unwrap();
    let takers = bodies_containing(&file_map, "binding_gate.try_");
    assert!(!takers.is_empty(), "no function takes the binding gate");
    for (name, body) in takers {
        assert!(
            !body.contains("yield_current") && !body.contains("spin_loop"),
            "{name} retries the binding gate on the CPU; a system call cannot be switched out there"
        );
        assert!(
            body.contains("prepare_to_wait_checked") && body.contains("wait_prepared_killable"),
            "{name} must sleep on the binding wait queue, checked under its lock"
        );
    }
    let releases = bodies_containing(&file_map, "binding_waiters.wake_up()");
    assert!(
        releases.iter().any(|(name, body)| *name == "drop" && body.contains("held.take()")),
        "dropping the gate guard must release the gate, then wake its waiters"
    );
}

#[test]
fn gate_waits_end_only_for_a_fatal_signal() {
    let blocking = fs::read_to_string(root().join("kernel/src/syscall/blocking_io.rs")).unwrap();
    let killable = bodies_containing(&blocking, "fatal_signal_pending");
    assert!(
        killable.iter().any(|(name, _)| *name == "wait_prepared_killable"),
        "exec must not fail with EINTR for a handled signal while it waits for the gate"
    );
}
