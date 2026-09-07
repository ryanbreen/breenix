//! Sampling ratchets for #855's aarch64 diagnostic ring providers.
//! Mutations operate on the real function bodies and reuse the real assertions.
use std::fs;
use std::path::PathBuf;

const SOURCE: &str = "kernel/src/arch_impl/aarch64/context_switch.rs";

fn read(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
    fs::read_to_string(&full)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", full.display()))
}

/// The body of the first `fn <name>(` in `source`, up to the closing brace at
/// the same indentation as the signature line. Copied from the identical
/// helper in tests/dispatch_path_lock_free_structure.rs; `trace_timer_tick`
/// is written at one indentation level, which is what makes that terminator
/// exact.
fn function_body(source: &str, name: &str) -> String {
    let needle = format!("fn {name}(");
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.contains(&needle))
        .unwrap_or_else(|| panic!("no `{needle}` in the source under test"));
    let indent = lines[start].len() - lines[start].trim_start().len();
    let terminator = format!("{}}}", " ".repeat(indent));
    let end = lines[start..]
        .iter()
        .position(|line| *line == terminator)
        .unwrap_or_else(|| panic!("no terminator for `{needle}`"))
        + start;
    lines[start..=end].join("\n")
}

fn compact(source: &str) -> String {
    source.chars().filter(|c| !c.is_whitespace()).collect()
}

// As in trace_ring_depth_structure, match the closing brace at the
// guard's indentation; whitespace-only indentation changes remain valid.
fn guard_blocks(body: &str, header: &str) -> Vec<(usize, usize)> {
    let mut offset = 0;
    let lines: Vec<&str> = body.lines().collect();
    let mut blocks = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if compact(line) == compact(header) {
            let indent = line.len() - line.trim_start().len();
            let mut end = offset + line.len() + 1;
            let mut closed = false;
            for next in &lines[index + 1..] {
                end += next.len() + 1;
                if next.trim() == "}" && next.len() - next.trim_start().len() == indent {
                    closed = true;
                    break;
                }
            }
            assert!(closed, "sampling guard must close");
            blocks.push((offset, end.min(body.len())));
        }
        offset += line.len() + 1;
    }
    blocks
}

fn assert_sampling_decision(body: &str, counter: &str, sample: &str) -> usize {
    let code = compact(body);
    let increment = code
        .find(&format!("{counter}[cpu_id].fetch_add(1,Ordering::Relaxed)"))
        .expect("each call must advance its relaxed per-CPU sampling counter");
    let mask = code
        .find(&format!("count&({sample}-1)==0"))
        .expect("sampling must use the named power-of-two mask");
    assert!(
        increment < mask,
        "the call counter must advance before sampling"
    );
    let decision = body
        .find("let sampled")
        .expect("sampling decision must exist");
    for (record, _) in body.match_indices("record_event(") {
        assert!(decision < record, "each ring write must follow sampling");
    }
    decision
}

fn assert_ctx_sampling(body: &str) {
    let decision = assert_sampling_decision(body, "CTX_DIAG_CALL_COUNT", "TRACE_DIAG_SAMPLE");
    let guards = guard_blocks(body, "if !sampled {");
    assert_eq!(
        guards.len(),
        1,
        "CTX sampling requires its early-return guard"
    );
    let (start, end) = guards[0];
    assert!(decision < start);
    let guard = compact(&body[start..end]);
    assert!(
        guard.contains("CTX_DIAG_SAMPLE_DROPPED.increment();return;"),
        "CTX sampling requires its counted early return"
    );
    assert!(body[..start].contains("fetch_add"));
    let records: Vec<_> = body.match_indices("record_event(").collect();
    assert_eq!(records.len(), 8, "keep all eight CTX ring writes");
    for (record, _) in records {
        assert!(
            end <= record,
            "CTX ring writes must follow the early-return guard"
        );
    }
}

fn assert_defer_sampling(body: &str) {
    assert_sampling_decision(body, "DEFER_REQUEUE_CALL_COUNT", "TRACE_DIAG_SAMPLE");
    let guards = guard_blocks(body, "if sampled {");
    let records: Vec<_> = body.match_indices("record_event(").collect();
    // Stage plus SP, ELR, X30 and FLAGS: five existing calls, despite the
    // issue's prose saying four in its ratchet checklist.
    assert_eq!(records.len(), 5, "keep all five DEFER ring writes");
    for (record, _) in records {
        assert!(
            guards
                .iter()
                .any(|(start, end)| *start < record && record < *end),
            "DEFER ring writes must remain inside sampled guards"
        );
    }
    assert!(
        !body.contains("return;"),
        "DEFER snapshots must not be skipped by an early return"
    );
    for name in ["INFO", "SP", "ELR", "X30"] {
        let needle = format!("LAST_DEFER_REQUEUE_{name}[cpu_id].store(");
        let code = compact(body);
        let store = code
            .find(&needle)
            .expect("most-recent snapshot store must remain");
        for (start, end) in guards
            .iter()
            .chain(guard_blocks(body, "if !sampled {").iter())
        {
            assert!(
                !(compact(&body[..*start]).len() < store && store < compact(&body[..*end]).len()),
                "snapshot stores must remain outside sampling guards"
            );
        }
    }
    let drops = guard_blocks(body, "if !sampled {");
    assert_eq!(drops.len(), 1, "DEFER skipped calls must be counted");
    let (start, end) = drops[0];
    assert!(compact(&body[start..end]).contains("DEFER_REQUEUE_SAMPLE_DROPPED.increment();"));
    assert_eq!(body.matches("Aarch64PerCpu::cpu_id()").count(), 1);
}

#[test]
fn sample_constant_is_named_and_asserted_power_of_two() {
    let source = compact(&read(SOURCE));
    assert!(source.contains("constTRACE_DIAG_SAMPLE:u64="));
    assert!(source.contains("const_:()=assert!(TRACE_DIAG_SAMPLE.is_power_of_two(),"));
}

#[test]
fn ctx_ring_writes_are_sampled() {
    assert_ctx_sampling(&function_body(&read(SOURCE), "trace_ctx_diag"));
}

#[test]
fn defer_ring_writes_are_sampled_and_snapshots_unconditional() {
    assert_defer_sampling(&function_body(&read(SOURCE), "trace_defer_requeue"));
}

#[test]
#[should_panic(expected = "CTX sampling requires its early-return guard")]
fn deleting_ctx_early_return_would_be_caught() {
    let body = function_body(&read(SOURCE), "trace_ctx_diag");
    assert_ctx_sampling(&body);
    let (start, end) = guard_blocks(&body, "if !sampled {")[0];
    let mutated = format!("{}{}", &body[..start], &body[end..]);
    assert_ctx_sampling(&mutated);
}

#[test]
#[should_panic(expected = "DEFER ring writes must remain inside sampled guards")]
fn deleting_defer_sampling_guard_would_be_caught() {
    let body = function_body(&read(SOURCE), "trace_defer_requeue");
    assert_defer_sampling(&body);
    let (start, end) = guard_blocks(&body, "if sampled {")[0];
    let opening_end = start + body[start..].find('{').unwrap() + 1;
    let closing = start + body[start..end].rfind('}').unwrap();
    let mutated = format!(
        "{}{}{}",
        &body[..start],
        &body[opening_end..closing],
        &body[end..]
    );
    assert_defer_sampling(&mutated);
}

#[test]
#[should_panic(expected = "snapshot stores must remain outside sampling guards")]
fn sampling_snapshot_stores_would_be_caught() {
    let body = function_body(&read(SOURCE), "trace_defer_requeue");
    assert_defer_sampling(&body);
    let snapshot = body
        .find("    if cpu_id < LAST_DEFER_REQUEUE_INFO.len() {")
        .unwrap();
    let end = snapshot + body[snapshot..].find("\n    }").unwrap() + "\n    }".len();
    let nested = body[snapshot..end]
        .lines()
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mutated = format!(
        "{}    if sampled {{\n{nested}\n    }}{}",
        &body[..snapshot],
        &body[end..]
    );
    assert_defer_sampling(&mutated);
}

const SCHED_SOURCE: &str = "kernel/src/task/scheduler.rs";

fn assert_sched_sampling(body: &str) {
    let decision = assert_sampling_decision(body, "SCHED_DIAG_CALL_COUNT", "TRACE_SCHED_DIAG_SAMPLE");
    let guards = guard_blocks(body, "if !sampled {");
    assert_eq!(guards.len(), 1, "SCHED sampling requires its early-return guard");
    let (start, end) = guards[0];
    assert!(decision < start);
    assert!(compact(&body[start..end]).contains("SCHED_DIAG_SAMPLE_DROPPED.increment();return;"));
    let records: Vec<_> = body.match_indices("record_event(").collect();
    assert_eq!(records.len(), 3, "keep all three SCHED ring writes");
    for (record, _) in records {
        assert!(end <= record, "SCHED ring writes must follow the early-return guard");
    }
}

#[test]
fn sched_sample_constant_is_named_and_asserted_power_of_two() {
    let source = compact(&read(SCHED_SOURCE));
    assert!(source.contains("constTRACE_SCHED_DIAG_SAMPLE:u64="));
    assert!(source.contains("const_:()=assert!(TRACE_SCHED_DIAG_SAMPLE.is_power_of_two(),"));
}

#[test]
fn sched_ring_writes_are_sampled() {
    assert_sched_sampling(&function_body(&read(SCHED_SOURCE), "trace_sched_diag"));
}

#[test]
#[should_panic(expected = "SCHED sampling requires its early-return guard")]
fn deleting_sched_early_return_would_be_caught() {
    let body = function_body(&read(SCHED_SOURCE), "trace_sched_diag");
    assert_sched_sampling(&body);
    let (start, end) = guard_blocks(&body, "if !sampled {")[0];
    let mutated = format!("{}{}", &body[..start], &body[end..]);
    assert_sched_sampling(&mutated);
}

const BUFFER_SOURCE: &str = "kernel/src/tracing/buffer.rs";

fn assert_buffer_size(source: &str) {
    assert!(
        source.lines().any(|line| line == "pub const TRACE_BUFFER_SIZE: usize = 2048;"),
        "trace buffer must retain the measured 2048-entry capacity"
    );
}

#[test]
fn buffer_size_is_pinned_to_measured_capacity() {
    assert_buffer_size(&read(BUFFER_SOURCE));
}

#[test]
#[should_panic(expected = "trace buffer must retain the measured 2048-entry capacity")]
fn reverting_buffer_size_would_be_caught() {
    let source = read(BUFFER_SOURCE);
    assert_buffer_size(&source);
    let mutated = source.replace(
        "pub const TRACE_BUFFER_SIZE: usize = 2048;",
        "pub const TRACE_BUFFER_SIZE: usize = 1024;",
    );
    assert_ne!(source, mutated);
    assert_buffer_size(&mutated);
}

#[test]
fn diagnostic_sample_values_are_pinned() {
    assert!(read(SOURCE).contains("const TRACE_DIAG_SAMPLE: u64 = 1024;"));
    assert!(read(SCHED_SOURCE).contains("const TRACE_SCHED_DIAG_SAMPLE: u64 = 8192;"));
}

fn assert_ctx_counters_are_padded_per_element(source: &str) {
    let code = compact(source);
    for name in ["CTX_DIAG_CALL_COUNT", "DEFER_REQUEUE_CALL_COUNT"] {
        assert!(code.contains(&format!("static{name}:[CacheLineAligned<AtomicU64>;")),
            "{name} must pad each element to its own cache line, not wrap the whole array");
    }
}

#[test]
fn ctx_and_defer_counters_are_padded_per_element() {
    assert_ctx_counters_are_padded_per_element(&read(SOURCE));
}

/// Replace the entire declaration with its pre-fix shared-array form.
fn revert_counter(source: &str, name: &str, old: &str) -> String {
    let start = source.find(&format!("static {name}:")).expect("counter declaration");
    let end = start + source[start..].find("MAX_CPUS];").expect("array initializer end")
        + "MAX_CPUS];".len();
    let mutated = format!("{}{}{}", &source[..start], old, &source[end..]);
    assert_ne!(source, mutated);
    mutated
}

#[test]
#[should_panic(expected = "must pad each element")]
fn reverting_ctx_counter_to_one_shared_line_would_be_caught() {
    let source = read(SOURCE);
    assert_ctx_counters_are_padded_per_element(&source);
    let mutated = revert_counter(&source, "CTX_DIAG_CALL_COUNT", r#"static CTX_DIAG_CALL_COUNT: CacheLineAligned<
    [AtomicU64; crate::arch_impl::aarch64::constants::MAX_CPUS],
> = CacheLineAligned([const { AtomicU64::new(0) }; crate::arch_impl::aarch64::constants::MAX_CPUS]);"#);
    assert_ctx_counters_are_padded_per_element(&mutated);
}

fn assert_sched_counter_is_padded_per_element(source: &str) {
    assert!(compact(source).contains("staticSCHED_DIAG_CALL_COUNT:[CacheLinePadded<AtomicU64>;"),
        "SCHED_DIAG_CALL_COUNT must pad each element to its own cache line");
}

#[test]
fn sched_counter_is_padded_per_element() {
    assert_sched_counter_is_padded_per_element(&read(SCHED_SOURCE));
}

#[test]
#[should_panic(expected = "must pad each element")]
fn reverting_sched_counter_to_one_shared_line_would_be_caught() {
    let source = read(SCHED_SOURCE);
    assert_sched_counter_is_padded_per_element(&source);
    let mutated = revert_counter(&source, "SCHED_DIAG_CALL_COUNT", r#"static SCHED_DIAG_CALL_COUNT: [AtomicU64; crate::arch_impl::aarch64::constants::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::arch_impl::aarch64::constants::MAX_CPUS];"#);
    assert_sched_counter_is_padded_per_element(&mutated);
}
