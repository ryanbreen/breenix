//! E2.2 (docs/plans/e2-sleepable-contexts.md §3.1-§3.2, §8): whether the
//! current context may sleep is decided in one place, `kernel/src/task/park*`.
//!
//! Before E2.2, each blocking primitive guessed from the preemption depth
//! whether its caller could sleep (`preempt_count() == 1` meant "a syscall",
//! `<= 1` meant "a syscall or a worker", `> 0` meant "holds a level I may
//! release"), and each one that parked from depth 0 first added a level so
//! that the sleep could remove one. These rules are censuses of the whole
//! kernel source, not lists of known names:
//!
//! * DEPTH: the comparisons of the preemption depth with 0 or 1, and the
//!   test of it against `PREEMPT_MASK`, outside the park module and the
//!   per-CPU count's own implementation. Each one must be classified below as
//!   not deciding whether to sleep, or as owned by a named later E2 PR. An
//!   unclassified comparison fails, and so does a classification that no
//!   longer matches anything, so the list only shrinks.
//! * LEVEL: the functions that both change the preemption depth and call a
//!   sleep primitive (the wait-queue's publication and sleep, the scheduler's
//!   blocked publication, or a halt or switch while blocked). The level
//!   arithmetic around a sleep belongs to the park module. The same rule for
//!   classification applies.
//!
//! On the tree before E2.2 both censuses find the predicates of the design's
//! §1.2 table and the add-a-level sites §3.2 names.
//!
//! Run: scripts/run-structure-tests.sh park_contract_structure

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Files that implement the count, the park module itself, and test-only
/// sources, which are not production sleep decisions.
fn excluded(rel: &str) -> bool {
    rel.starts_with("kernel/src/task/park")
        || rel == "kernel/src/per_cpu.rs"
        || rel == "kernel/src/per_cpu_aarch64.rs"
        || rel.ends_with("/percpu.rs")
        || rel.ends_with("_test.rs")
        || rel.ends_with("_tests.rs")
        || rel.contains("/test_framework/")
        || rel.contains("_oracle")
        || rel.ends_with("/lock_race.rs")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The source with `//` comments blanked, so prose that quotes a rule does
/// not count as code. Byte offsets are preserved.
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.split_inclusive('\n') {
        match line.find("//") {
            Some(at) if !line[..at].contains('"') => {
                out.push_str(&line[..at]);
                out.extend(line[at..].chars().map(|c| if c == '\n' { '\n' } else { ' ' }));
            }
            _ => out.push_str(line),
        }
    }
    out
}

/// The `fn` items in `code`, each with the byte range of its body.
fn functions(code: &str) -> Vec<(String, usize, usize)> {
    let bytes = code.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(off) = code[at..].find("fn ") {
        let start = at + off;
        at = start + 3;
        if start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
            continue;
        }
        let name: String = code[at..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        if name.is_empty() {
            continue;
        }
        // The body opens at the first `{` after the signature, unless a `;`
        // (a declaration without a body) comes first.
        let rest = &code[at..];
        let (Some(brace), semi) = (rest.find('{'), rest.find(';')) else { continue };
        if semi.is_some_and(|semi| semi < brace) {
            continue;
        }
        let open = at + brace;
        let mut depth = 0usize;
        let mut end = open;
        for (i, b) in bytes[open..].iter().enumerate() {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        found.push((name, open, end));
    }
    found
}

/// The innermost function whose body holds byte `pos`.
fn enclosing(fns: &[(String, usize, usize)], pos: usize) -> String {
    fns.iter()
        .filter(|(_, open, end)| *open <= pos && pos <= *end)
        .min_by_key(|(_, open, end)| end - open)
        .map(|(name, _, _)| name.clone())
        .unwrap_or_else(|| "<file scope>".to_string())
}

/// Whether the text right after a `preempt_count()` call compares it with 0
/// or 1, or tests it against `PREEMPT_MASK`.
fn compares_depth(after: &str) -> bool {
    let tail: String = after.chars().take(90).collect();
    let tail = tail.split([';', '{', '\n']).next().unwrap_or("").replace(' ', "");
    if tail.contains("PREEMPT_MASK") {
        return true;
    }
    let ops = ["==0", "==1", "!=0", "!=1", "<=1", ">0", ">=1", "<1"];
    ops.iter().any(|op| {
        tail.find(op).is_some_and(|i| {
            let next = tail.as_bytes().get(i + op.len()).copied();
            !next.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'x' || b == b'_')
        })
    })
}

const SLEEP_CALLS: [&str; 9] = [
    "prepare_to_wait(",
    "prepare_to_wait_checked(",
    "schedule_current_wait(",
    "block_current_for_io(",
    "block_current_for_io_with_timeout(",
    "halt_blocked_current(",
    "arch_halt_while_blocked(",
    "arch_halt_with_interrupts(",
    "schedule_from_kernel(",
];

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
struct Site {
    rule: &'static str,
    file: String,
    function: String,
}

fn census(root: &Path) -> Vec<Site> {
    let mut files = Vec::new();
    rust_files(&root.join("kernel/src"), &mut files);
    let mut sites = Vec::new();
    for path in files {
        let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if excluded(&rel) {
            continue;
        }
        let code = strip_comments(&fs::read_to_string(&path).unwrap());
        let fns = functions(&code);
        let mut at = 0;
        while let Some(off) = code[at..].find("preempt_count()") {
            let pos = at + off;
            at = pos + "preempt_count()".len();
            if compares_depth(&code[at..]) {
                sites.push(Site { rule: "DEPTH", file: rel.clone(), function: enclosing(&fns, pos) });
            }
        }
        for (name, open, end) in &fns {
            let body = &code[*open..=*end];
            // Only the function's own statements: a nested fn is its own entry.
            let nested: Vec<_> = fns.iter().filter(|(_, o, e)| o > open && e < end).collect();
            let own = |needle: &str| {
                let mut from = 0;
                while let Some(i) = body[from..].find(needle) {
                    let p = *open + from + i;
                    if !nested.iter().any(|(_, o, e)| *o <= p && p <= *e) {
                        return true;
                    }
                    from += i + needle.len();
                }
                false
            };
            let changes_level = own("preempt_disable()") || own("preempt_enable()");
            let sleeps = SLEEP_CALLS.iter().any(|call| own(&format!(".{call}")) || own(&format!("::{call}")) || own(&format!(" {call}")) || own(&format!("\t{call}")));
            if changes_level && sleeps {
                sites.push(Site { rule: "LEVEL", file: rel.clone(), function: name.clone() });
            }
        }
    }
    sites.sort();
    sites.dedup();
    sites
}

/// Comparisons and level changes that are not the park contract's business,
/// or that a later E2 PR owns. (rule, file, function, why).
const CLASSIFIED: &[(&str, &str, &str, &str)] = &[
    // Not a decision about sleeping.
    ("DEPTH", "kernel/src/main.rs", "kernel_main_continue", "boot precondition log: the boot brake is held"),
    ("DEPTH", "kernel/src/serial.rs", "_log_print", "serial back-pressure: busy-waits for log room, never parks"),
    ("DEPTH", "kernel/src/net/mod.rs", "drop", "runs pending softirqs on the last release, as per_cpu does"),
    ("DEPTH", "kernel/src/task/scheduler.rs", "note_x86_interrupt_return", "diagnostic: records a dispatchable return tick"),
    ("DEPTH", "kernel/src/task/softirqd.rs", "init_online_daemons", "boot order: CPU 0 is still pinned by kernel_main"),
    ("DEPTH", "kernel/src/arch_impl/aarch64/context_switch.rs", "can_dispatch_here", "the scheduler's own dispatch gate on depth; the exit's decision about the thread is park_core::exit_switch (E2.3)"),
    // Syscall waits that release the syscall's level by hand. Each syscall
    // class moves onto park::wait when E2.10 makes it preemptible (design
    // §4.4), because at baseline 0 the hand release would underflow.
    ("LEVEL", "kernel/src/task/thread.rs", "enter_syscall", "E2.10a: the custody park"),
    ("LEVEL", "kernel/src/syscall/handlers.rs", "read_pipe", "E2.10b: file and pipe I/O"),
    ("LEVEL", "kernel/src/syscall/handlers.rs", "sys_read", "E2.10b: file and pipe I/O"),
    ("LEVEL", "kernel/src/syscall/fs.rs", "handle_fifo_open", "E2.10b: file and pipe I/O"),
    ("LEVEL", "kernel/src/syscall/futex.rs", "futex_lock_pi", "E2.10c: waits"),
    ("LEVEL", "kernel/src/syscall/futex.rs", "futex_wait", "E2.10c: waits"),
    ("LEVEL", "kernel/src/syscall/futex.rs", "futex_wait_until", "E2.10c: waits"),
    ("LEVEL", "kernel/src/syscall/multiplex.rs", "park", "E2.10c: waits (poll, select)"),
    ("LEVEL", "kernel/src/syscall/sleep.rs", "clock_nanosleep", "E2.10c: waits"),
    ("LEVEL", "kernel/src/socket/wait.rs", "wait_until", "E2.10c: waits (sockets)"),
    ("LEVEL", "kernel/src/syscall/signal.rs", "sys_sigtimedwait", "E2.10e: signal syscalls"),
    ("LEVEL", "kernel/src/syscall/signal.rs", "wait_for_signal", "E2.10e: signal syscalls"),
    ("LEVEL", "kernel/src/syscall/wait.rs", "wait_for_child", "E2.10e: wait4 and waitid"),
    ("LEVEL", "kernel/src/signal/delivery.rs", "abandon_syscall_return", "E2.10e: the stop and signal machinery"),
    ("LEVEL", "kernel/src/signal/delivery.rs", "exit_on_syscall_return", "E2.10e: the stop and signal machinery"),
    ("LEVEL", "kernel/src/signal/delivery.rs", "hold_stopped_thread_on_syscall_return", "E2.10e: the stop and signal machinery"),
];

#[test]
fn every_depth_test_and_level_change_around_a_sleep_is_classified() {
    let root = repo_root();
    let sites = census(&root);
    let classified = |site: &Site| {
        CLASSIFIED.iter().any(|(rule, file, function, _)| *rule == site.rule && *file == site.file && *function == site.function)
    };
    let unclassified: Vec<_> = sites.iter().filter(|site| !classified(site)).collect();
    assert!(
        unclassified.is_empty(),
        "sleep decisions or wait level arithmetic outside kernel/src/task/park*:\n{}",
        unclassified.iter().map(|s| format!("  {} {}::{}", s.rule, s.file, s.function)).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn every_classification_still_matches_a_site() {
    let root = repo_root();
    let sites = census(&root);
    let stale: Vec<_> = CLASSIFIED
        .iter()
        .filter(|(rule, file, function, _)| !sites.iter().any(|s| s.rule == *rule && s.file == *file && s.function == *function))
        .collect();
    assert!(stale.is_empty(), "classifications that match nothing (remove them): {stale:?}");
}

// ------------------------------------------------- halting as current (E2.3)

/// The calls that publish the current thread blocked.
const PUBLISH_CALLS: [&str; 9] = [
    "block_current(",
    "block_current_for_timer(",
    "block_current_for_io(",
    "block_current_for_io_with_timeout(",
    "block_current_for_signal(",
    "block_current_for_signal_with_context(",
    "block_current_for_child_exit(",
    "prepare_to_wait(",
    "prepare_to_wait_checked(",
];

/// The ways a thread halts or WFIs as the current thread: it keeps its CPU
/// until the next interrupt.
const HALTS: [&str; 3] = ["arch_halt_with_interrupts(", "enable_and_hlt(", "\"wfi\""];

/// The byte offsets in `code` at which the body of the function spanning
/// `open..=end` calls `call` itself, outside any function nested in it.
fn own_calls(code: &str, fns: &[(String, usize, usize)], open: usize, end: usize, call: &str) -> Vec<usize> {
    let body = &code[open..=end];
    let nested: Vec<_> = fns.iter().filter(|(_, o, e)| *o > open && *e < end).collect();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(i) = body[from..].find(call) {
        let p = open + from + i;
        let before = code.as_bytes()[p - 1];
        let starts_call = !(before.is_ascii_alphanumeric() || before == b'_');
        if starts_call && !nested.iter().any(|(_, o, e)| *o <= p && p <= *e) {
            found.push(p);
        }
        from += i + call.len();
    }
    found
}

/// Whether the function spanning `open..=end` publishes the current thread
/// blocked and halts after that publication.
fn halts_after_publishing(code: &str, fns: &[(String, usize, usize)], open: usize, end: usize) -> bool {
    let first_publish =
        PUBLISH_CALLS.iter().flat_map(|call| own_calls(code, fns, open, end, call)).min();
    first_publish.is_some_and(|published| {
        HALTS.iter().flat_map(|halt| own_calls(code, fns, open, end, halt)).any(|halt| halt > published)
    })
}

/// The task-context waits that halt as the current thread: functions that
/// publish the current thread blocked and then halt or WFI themselves,
/// rather than sleeping through `arch_halt_while_blocked`, which switches
/// away whenever the thread may be switched (design §3.5). A halt before
/// the publication (a fallback for a thread with no scheduler identity yet)
/// is not a wait.
fn halt_census(root: &Path) -> Vec<Site> {
    let mut files = Vec::new();
    rust_files(&root.join("kernel/src"), &mut files);
    let mut sites = Vec::new();
    for path in files {
        let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if excluded(&rel) {
            continue;
        }
        let code = strip_comments(&fs::read_to_string(&path).unwrap());
        let fns = functions(&code);
        for (name, open, end) in &fns {
            if halts_after_publishing(&code, &fns, *open, *end) {
                sites.push(Site { rule: "HALT", file: rel.clone(), function: name.clone() });
            }
        }
    }
    sites.sort();
    sites.dedup();
    sites
}

/// Kernel threads that still publish a timer or plain block, set a
/// reschedule, and halt as the current thread until the next interrupt's exit
/// switches them out. (file, function, why). E2.3 converted these 8 to sleep
/// through `arch_halt_while_blocked`; when that tree's ARM64 boot soft-locked
/// they were the suspect and came back out, and the next boot showed the cause
/// was elsewhere (the pty hangup daemon's bare brake). No tree with the
/// conversion and that fix was booted, so each moves in a later PR that gets
/// its own boot (docs/plans/e2-sleepable-contexts.md, "E2.3 as built"). The
/// list only shrinks: an entry that matches no site fails
/// `every_halt_classification_still_matches_a_site`.
const HALT_CLASSIFIED: &[(&str, &str, &str)] = &[
    ("kernel/src/console/mod.rs", "sleep_ms", "x86 render-thread timer sleep"),
    ("kernel/src/drivers/watchdog_round.rs", "wait", "x86 watchdog round wait"),
    ("kernel/src/gpu/gk208/production.rs", "guard_thread", "GT 710 clock-guard poll"),
    ("kernel/src/net/loopback_pump.rs", "loopback_pump_fn", "kloopbackd plain block"),
    ("kernel/src/net/rx_poll.rs", "sleep_one_tick", "ARM64 receive-poll one-tick sleep"),
    ("kernel/src/net/tcp.rs", "tcp_timer_thread", "ktcptimerd timer or plain block"),
    ("kernel/src/task/dispatch_strand_census.rs", "sleep_one_interval", "kstrandd interval sleep"),
    ("kernel/src/task/userspace_completion.rs", "report_when_finished", "testing completion reporter poll"),
];

/// Halting as current survives only where it is the same as running idle
/// (design §3.5): in the idle thread and early boot, which are not waits that
/// publish a blocked state, and in `arch_halt_while_blocked`'s own branches.
/// No task-context wait outside `HALT_CLASSIFIED` halts as current on its own.
#[test]
fn no_task_context_wait_halts_as_current() {
    let sites = halt_census(&repo_root());
    let unclassified: Vec<_> = sites
        .iter()
        .filter(|s| !HALT_CLASSIFIED.iter().any(|(file, function, _)| s.file == *file && s.function == *function))
        .collect();
    assert!(
        unclassified.is_empty(),
        "task-context waits that publish a blocked state and then halt as the current thread:\n{}",
        unclassified.iter().map(|s| format!("  {}::{}", s.file, s.function)).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn every_halt_classification_still_matches_a_site() {
    let sites = halt_census(&repo_root());
    let stale: Vec<_> = HALT_CLASSIFIED
        .iter()
        .filter(|(file, function, _)| !sites.iter().any(|s| s.file == *file && s.function == *function))
        .collect();
    assert!(stale.is_empty(), "halt classifications that match nothing (remove them): {stale:?}");
}

/// The `Completion` waits, which sleep only at a braked baseline
/// (`park::sleepable_braked`) and otherwise poll the CPU until their deadline.
const COMPLETION_WAITS: [&str; 3] = [".wait_timeout(", ".wait_timeout_uninterruptible(", ".wait_for_completion("];

/// Functions that take a bare `preempt_disable()` and then wait on a
/// `Completion`: a kernel thread faking a syscall's brake to select the
/// sleeping wait. Since E2.2 that level is `LocksHeld(1)` at baseline 0, so the
/// wait polls its CPU for its whole timeout instead of sleeping; a kernel
/// thread that keeps its CPU except while it sleeps holds `park::Braked`
/// (design §3.2).
fn bare_brake_census(root: &Path) -> Vec<Site> {
    let mut files = Vec::new();
    rust_files(&root.join("kernel/src"), &mut files);
    let mut sites = Vec::new();
    for path in files {
        let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if excluded(&rel) || rel == "kernel/src/task/completion.rs" {
            continue;
        }
        let code = strip_comments(&fs::read_to_string(&path).unwrap());
        let fns = functions(&code);
        for (name, open, end) in &fns {
            let brakes = own_calls(&code, &fns, *open, *end, "preempt_disable()");
            let first_brake = brakes.iter().min();
            let waits_after = COMPLETION_WAITS
                .iter()
                .flat_map(|wait| own_calls(&code, &fns, *open, *end, &wait[1..]))
                .filter(|w| code.as_bytes()[w - 1] == b'.')
                .any(|w| first_brake.is_some_and(|b| w > *b));
            if waits_after {
                sites.push(Site { rule: "BRAKE", file: rel.clone(), function: name.clone() });
            }
        }
    }
    sites.sort();
    sites.dedup();
    sites
}

/// A kernel thread that waits on a `Completion` as a syscall does holds a
/// `park::Braked` guard, not a bare `preempt_disable()`.
#[test]
fn no_kernel_thread_fakes_a_brake_for_a_completion_wait() {
    let sites = bare_brake_census(&repo_root());
    assert!(
        sites.is_empty(),
        "bare preempt_disable() before a Completion wait (hold park::Braked instead):\n{}",
        sites.iter().map(|s| format!("  {}::{}", s.file, s.function)).collect::<Vec<_>>().join("\n")
    );
}

/// A syscall runs at its braked baseline. A halt as the current thread does
/// not release that level, so the interrupt that ends it cannot switch the
/// thread away, and no receipt records it. A syscall that backs off uses
/// `task::park::retry_pause`, which releases the level for the halt and
/// records a caller it still cannot switch from; one that waits uses
/// `park::wait`.
#[test]
fn no_syscall_halts_as_current_itself() {
    let root = repo_root();
    let mut files = Vec::new();
    rust_files(&root.join("kernel/src/syscall"), &mut files);
    let mut sites = Vec::new();
    for path in files {
        let rel = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        let code = strip_comments(&fs::read_to_string(&path).unwrap());
        let fns = functions(&code);
        for (name, open, end) in &fns {
            if HALTS.iter().any(|halt| !own_calls(&code, &fns, *open, *end, halt).is_empty()) {
                sites.push(format!("  {rel}::{name}"));
            }
        }
    }
    sites.sort();
    sites.dedup();
    assert!(
        sites.is_empty(),
        "syscall paths that halt as the current thread at their braked baseline (use park::retry_pause):\n{}",
        sites.join("\n")
    );
}

/// `park::retry_pause` releases one level for its halt. A caller that holds
/// more is still not preemptible there, and keeps its CPU until the
/// interrupt: that halt records a receipt at the caller's site, as
/// `arch_halt_while_blocked`'s do (design §3.5).
#[test]
fn retry_pause_records_a_caller_it_cannot_switch_from() {
    let code = strip_comments(&fs::read_to_string(repo_root().join("kernel/src/task/park.rs")).unwrap());
    let fns = functions(&code);
    let (_, open, end) = fns.iter().find(|(name, _, _)| name == "retry_pause").expect("retry_pause");
    let halt = own_calls(&code, &fns, *open, *end, "arch_halt_with_interrupts(");
    assert_eq!(halt.len(), 1, "one halt");
    let gated = own_calls(&code, &fns, *open, *end, "may_switch_now(");
    let receipts = own_calls(&code, &fns, *open, *end, "halt_as_current(");
    assert!(
        receipts.iter().any(|r| gated.iter().any(|g| g < r) && *r < halt[0]),
        "retry_pause halts with no halt_as_current() receipt, gated on may_switch_now(), before it"
    );
    let head = &code[..*open];
    let signature = head.rfind("fn retry_pause").expect("retry_pause signature");
    assert!(
        head[..signature].trim_end().trim_end_matches("pub(crate)").trim_end().ends_with("#[track_caller]"),
        "retry_pause is #[track_caller], so its receipt names the caller's site"
    );
}

#[test]
fn the_brake_census_sees_each_shape() {
    let code = "fn a() { crate::per_cpu::preempt_disable(); let _ = k.wait_timeout_uninterruptible(1, 2); }\n\
                fn b() { let _g = crate::task::park::Braked::new(); let _ = k.wait_timeout_uninterruptible(1, 2); }\n\
                fn c() { let _ = k.wait_timeout(1, 2); preempt_disable(); }\n\
                fn d() { preempt_disable(); c.wait_for_completion(); }\n";
    let fns = functions(code);
    let hits: Vec<_> = fns
        .iter()
        .filter(|(_, open, end)| {
            let first_brake = own_calls(code, &fns, *open, *end, "preempt_disable()").into_iter().min();
            COMPLETION_WAITS
                .iter()
                .flat_map(|wait| own_calls(code, &fns, *open, *end, &wait[1..]))
                .filter(|w| code.as_bytes()[w - 1] == b'.')
                .any(|w| first_brake.is_some_and(|b| w > b))
        })
        .map(|(name, _, _)| name.as_str())
        .collect();
    assert_eq!(hits, ["a", "d"], "a bare brake before a Completion wait is found; a Braked guard or a wait before the brake is not");
}

/// `arch_halt_while_blocked` is where halting as current survives. Each of
/// its halts (x86's HLT, ARM64's WFI) is preceded, after the previous halt,
/// by `task::park::halt_as_current`, which records a non-preemptible
/// caller's halt as a contract violation and lets the idle thread and early
/// boot through (design §3.5).
#[test]
fn each_halt_as_current_records_a_non_preemptible_caller() {
    let path = repo_root().join("kernel/src/lib.rs");
    let code = strip_comments(&fs::read_to_string(path).unwrap());
    let fns = functions(&code);
    let (_, open, end) = fns.iter().find(|(name, _, _)| name == "arch_halt_while_blocked").expect("arch_halt_while_blocked");
    let mut halts: Vec<usize> =
        ["enable_and_hlt(", "\"wfi\""].iter().flat_map(|halt| own_calls(&code, &fns, *open, *end, halt)).collect();
    halts.sort();
    assert_eq!(halts.len(), 2, "one HLT and one WFI");
    let receipts = own_calls(&code, &fns, *open, *end, "halt_as_current(");
    let mut previous = *open;
    for halt in halts {
        assert!(
            receipts.iter().any(|r| previous < *r && *r < halt),
            "a halt as the current thread at byte {halt} has no halt_as_current() receipt before it"
        );
        previous = halt;
    }
}

#[test]
fn the_halt_census_sees_each_shape() {
    let code = "fn a() { s.block_current_for_timer(t); loop { crate::arch_halt_with_interrupts(); } }\n\
                fn b() { s.block_current(); x86_64::instructions::interrupts::enable_and_hlt(); }\n\
                fn c() { q.prepare_to_wait_checked(s, d, || true); asm!(\"wfi\"); }\n\
                fn d() { s.block_current_for_timer(t); crate::arch_halt_while_blocked(); }\n\
                fn e() { crate::arch_halt_with_interrupts(); }\n\
                fn f() { s.block_current(); fn g() { crate::arch_halt_with_interrupts(); } }\n\
                fn h() { let Some(t) = id() else { crate::arch_halt_with_interrupts(); return; }; s.block_current_for_timer(t); }\n";
    let fns = functions(code);
    let hits: Vec<_> = fns
        .iter()
        .filter(|(_, open, end)| halts_after_publishing(code, &fns, *open, *end))
        .map(|(name, _, _)| name.as_str())
        .collect();
    assert_eq!(
        hits,
        ["a", "b", "c"],
        "a publication followed by its own halt is found; a switch, a bare halt, a nested fn and a halt before publishing are not"
    );
}

#[test]
fn the_census_sees_each_shape() {
    let code = "fn a() { if preempt_count() <= 1 { } }\nfn b() { let x = cpu::preempt_count() & PREEMPT_MASK != 0; }\n\
                fn c() { preempt_disable(); q.prepare_to_wait_checked(s, d, || true); }\n\
                fn d() { if preempt_count() == 0x10 {} let y = preempt_count() == 10; }\n";
    let fns = functions(code);
    let mut hits = Vec::new();
    let mut at = 0;
    while let Some(off) = code[at..].find("preempt_count()") {
        let pos = at + off;
        at = pos + "preempt_count()".len();
        if compares_depth(&code[at..]) {
            hits.push(enclosing(&fns, pos));
        }
    }
    assert_eq!(hits, ["a", "b"], "a depth comparison with 0/1 or PREEMPT_MASK is found, other values are not");
    let (_, open, end) = fns.iter().find(|(n, _, _)| n == "c").unwrap();
    let body = &code[*open..=*end];
    assert!(body.contains("preempt_disable()") && body.contains(".prepare_to_wait_checked("));
}
