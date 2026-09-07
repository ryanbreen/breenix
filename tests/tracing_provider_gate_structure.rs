//! Direct x86 provider wiring and the shipping gate's actual shell oracle.
//! Textual shape checks deliberately pin this small wrapper and bare shell
//! block; mutations exercise the same assertions against real source copies.
use std::{fs, path::PathBuf, process::Command};

const REGISTRY: &str = "kernel/src/test_framework/registry.rs";
const MAIN: &str = "kernel/src/main.rs";
const SCRIPT: &str = "docker/qemu/run-x86-boot-tests.sh";
const CFG: &str = "#[cfg(all(target_arch = \"x86_64\", feature = \"boot_tests\"))]";
const HEADER: &str = "pub fn run_x86_tracing_provider_gate() {";
const CALL: &str = "kernel::test_framework::registry::run_x86_tracing_provider_gate();";
const START: &str = "[TEST:process:deferred_fault_ring_overflow_injection:START]";
const PASS: &str = "[TEST:process:deferred_fault_ring_overflow_injection:PASS]";
const FAIL: &str = "[TEST:process:deferred_fault_ring_overflow_injection:FAIL:";
const RING_END: &str = "    test \"$RING_SPAN_TICKS_TOTAL\" -ge \"$((RING_SPAN_TICK_EVENTS * RING_SPAN_RATIO_FLOOR))\"\n";
const NEXT: &str = "    # (6) #766";

fn read(path: &str) -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

fn item_body<'a>(source: &'a str, header: &str) -> &'a str {
    let start = source.find(header).expect("item header exists");
    let end = start + source[start..].find("\n}").expect("item closes") + 2;
    &source[start..end]
}

fn compact(source: &str) -> String {
    source.chars().filter(|c| !c.is_whitespace()).collect()
}

fn assert_result_controls_markers(body: &str) {
    // Pin the entire small body, so neither an unconditional PASS outside
    // the match nor a branch that ignores the provider result is accepted.
    let expected = format!(
        r#"{HEADER}
        crate::serial_println!("{START}");
        let result = crate::tracing::providers::teardown::deferred_fault_ring_overflow_test();
        match result {{
            TestResult::Pass => crate::serial_println!("{PASS}"),
            _ => crate::serial_println!("{FAIL}{{}}]",
                result.failure_message().unwrap_or("test failed")),
        }}
    }}"#
    );
    assert_eq!(
        compact(body),
        compact(&expected),
        "PASS must depend on TestResult"
    );
}

fn assert_wrapper(source: &str) {
    assert!(
        source.contains(&format!("{CFG}\n{HEADER}")),
        "wrapper cfg must be exactly shipping boot_tests"
    );
    assert_result_controls_markers(item_body(source, HEADER));
}

fn assert_call(source: &str) {
    let call = source.find(CALL).expect("direct call site must exist");
    assert!(
        source.contains(&format!("{CFG}\n    {CALL}")),
        "call cfg must be exactly shipping boot_tests"
    );
    let ring = source
        .find("kernel::test_framework::registry::run_x86_ring_span_gate();")
        .unwrap();
    let disable = ring
        + source[ring..]
            .find("x86_64::instructions::interrupts::disable();")
            .unwrap();
    assert!(
        ring < call && call < disable,
        "call must follow ring-span before nearest interrupt disable"
    );
}

fn registration(source: &str) -> &str {
    let name = source
        .find("name: \"deferred_fault_ring_overflow_injection\"")
        .unwrap();
    let start = source[..name].rfind("    TestDef {").unwrap();
    let end = name + source[name..].find("\n    },").unwrap() + "\n    },".len();
    &source[start..end]
}

fn assert_registration(block: &str) {
    assert!(
        block.contains("arch: Arch::Aarch64,") && !block.contains("Arch::Any"),
        "registry copy must be aarch64 only"
    );
}

fn gate_block(source: &str) -> &str {
    let start = source.find(RING_END).expect("ring assertion exists") + RING_END.len();
    let end = start + source[start..].find(NEXT).expect("next assertion exists");
    &source[start..end]
}

fn assert_gate(source: &str) {
    let block = gate_block(source);
    let code = block
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        code.contains("test -s \"$OUTPUT_DIR/serial_user.txt\"")
            && code.contains("awk '")
            && [START, PASS, FAIL].iter().all(|m| code.contains(m)),
        "gate must require provider marker block"
    );
    assert!(!code.contains("serial_*.txt"), "provider scores COM1 only");
    assert!(
        !code
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|token| token == "if"),
        "gate assertions must be bare, without if"
    );
    assert!(
        code.trim_start().starts_with("test -s ")
            && code
                .trim_end()
                .ends_with("' \"$OUTPUT_DIR/serial_user.txt\""),
        "gate assertions must be bare"
    );
    assert!(
        code.contains("END { exit !(started == 1 && passed == 1 && failed == 0) }"),
        "gate predicate must require exactly one start, one pass, zero fail"
    );
}

#[test]
fn wrapper_uses_shipping_cfg_and_result_dependent_markers() {
    assert_wrapper(&read(REGISTRY));
}

#[test]
#[should_panic(expected = "PASS must depend on TestResult")]
fn printing_pass_unconditionally_would_be_caught() {
    let source = read(REGISTRY);
    let body = item_body(&source, HEADER);
    assert_result_controls_markers(body);
    let prefix = &body[..body.find("    match result {").unwrap()];
    let mutated = format!("{prefix}    crate::serial_println!(\"{PASS}\");\n}}");
    assert_result_controls_markers(&mutated);
}

#[test]
#[should_panic(expected = "wrapper cfg must be exactly shipping boot_tests")]
fn gating_wrapper_on_staged_executor_would_be_caught() {
    let source = read(REGISTRY);
    assert_wrapper(&source);
    assert_wrapper(&source.replace(&format!("{CFG}\n{HEADER}"), &format!("#[cfg(all(target_arch = \"x86_64\", feature = \"boot_tests\", feature = \"x86_staged_registry\"))]\n{HEADER}")));
}

#[test]
fn direct_call_is_after_ring_span_before_next_disable_with_shipping_cfg() {
    assert_call(&read(MAIN));
}

#[test]
#[should_panic(expected = "direct call site must exist")]
fn removing_direct_call_would_be_caught() {
    let source = read(MAIN);
    assert_call(&source);
    assert_call(&source.replace(&format!("    {CALL}\n"), ""));
}

#[test]
#[should_panic(expected = "call cfg must be exactly shipping boot_tests")]
fn gating_direct_call_on_staged_executor_would_be_caught() {
    let source = read(MAIN);
    assert_call(&source);
    assert_call(&source.replace(&format!("{CFG}\n    {CALL}"), &format!("#[cfg(all(target_arch = \"x86_64\", feature = \"boot_tests\", feature = \"x86_staged_registry\"))]\n    {CALL}")));
}

#[test]
#[should_panic(expected = "call must follow ring-span before nearest interrupt disable")]
fn moving_direct_call_past_nearest_disable_would_be_caught() {
    let source = read(MAIN);
    assert_call(&source);
    let moved = source.replace(&format!("    {CFG}\n    {CALL}\n"), "");
    let ring = moved
        .find("kernel::test_framework::registry::run_x86_ring_span_gate();")
        .unwrap();
    let end = ring
        + moved[ring..]
            .find("x86_64::instructions::interrupts::disable();")
            .unwrap()
        + "x86_64::instructions::interrupts::disable();".len();
    let mutated = format!("{}\n    {CFG}\n    {CALL}{}", &moved[..end], &moved[end..]);
    assert_call(&mutated);
}

#[test]
fn registry_dispatches_provider_only_on_aarch64() {
    assert_registration(registration(&read(REGISTRY)));
}

#[test]
#[should_panic(expected = "registry copy must be aarch64 only")]
fn restoring_arch_any_duplicate_registration_would_be_caught() {
    let source = read(REGISTRY);
    let block = registration(&source);
    assert_registration(block);
    assert_registration(&block.replace("arch: Arch::Aarch64,", "arch: Arch::Any,"));
}

#[test]
fn gate_requires_provider_markers_from_com1_with_bare_assertions() {
    assert_gate(&read(SCRIPT));
}

#[test]
#[should_panic(expected = "gate must require provider marker block")]
fn removing_provider_gate_block_would_be_caught() {
    let source = read(SCRIPT);
    assert_gate(&source);
    assert_gate(&source.replace(gate_block(&source), ""));
}

#[test]
#[should_panic(expected = "gate assertions must be bare, without if")]
fn wrapping_gate_in_if_would_be_caught() {
    let source = read(SCRIPT);
    assert_gate(&source);
    let block = gate_block(&source);
    assert_gate(&source.replace(block, &format!("if true; then\n{block}\nfi\n")));
}

#[test]
#[should_panic(
    expected = "gate predicate must require exactly one start, one pass, zero fail"
)]
fn weakening_started_equality_would_be_caught() {
    let source = read(SCRIPT);
    assert_gate(&source);
    let block = gate_block(&source);
    let mutated = block.replace("started == 1", "started >= 1");
    assert_gate(&source.replace(block, &mutated));
}

#[test]
#[should_panic(
    expected = "gate predicate must require exactly one start, one pass, zero fail"
)]
fn weakening_failed_equality_would_be_caught() {
    let source = read(SCRIPT);
    assert_gate(&source);
    let block = gate_block(&source);
    let mutated = block.replace("failed == 0", "failed <= 1");
    assert_gate(&source.replace(block, &mutated));
}

fn unique_temp_dir(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "tracing-provider-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn run_oracle(tag: &str, serial: &str, expected: bool) {
    let source = read(SCRIPT);
    assert_gate(&source);
    let dir = unique_temp_dir(tag);
    fs::write(dir.join("serial_user.txt"), serial).unwrap();
    let output = Command::new("bash")
        .arg("-c")
        .arg(format!("set -e\n{}", gate_block(&source)))
        .env("OUTPUT_DIR", &dir)
        .output()
        .unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert_eq!(output.status.success(), expected, "{tag}: {output:?}");
}

#[test]
fn archived_baseline_without_provider_markers_is_rejected() {
    run_oracle("baseline", "[BOOT_TESTS:PASS]\nunrelated\n", false);
}
#[test]
fn exactly_one_start_and_pass_is_accepted() {
    run_oracle("positive", &format!("{START}\n{PASS}\n"), true);
}
#[test]
fn duplicate_pass_is_rejected() {
    run_oracle(
        "duplicate-pass",
        &format!("{START}\n{PASS}\n{PASS}\n"),
        false,
    );
}
#[test]
fn missing_start_is_rejected() {
    run_oracle("missing-start", &format!("{PASS}\n"), false);
}
#[test]
fn fail_alongside_pass_is_rejected() {
    run_oracle(
        "fail",
        &format!("{START}\n{PASS}\n{FAIL}something]\n"),
        false,
    );
}
