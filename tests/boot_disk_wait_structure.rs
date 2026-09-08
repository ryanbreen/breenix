//! Issue 508: idle boot cannot donate its continuation to ordinary scheduling.
use std::{fs, path::PathBuf};
fn read(path: &str) -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}
#[test]
fn boot_completion_preserves_brake_and_uses_atomic_check_halt() {
    let source = read("kernel/src/task/completion.rs");
    let wait = source
        .split("fn wait_idle_completion(")
        .nth(1)
        .expect("idle completion wait missing")
        .split("/// Completion primitive")
        .next()
        .unwrap();
    assert!(wait.contains("preempt_disable()"));
    assert!(wait.contains("without_interrupts(||"));
    assert!(wait.contains("self.done") || wait.contains("completion.done"));
    assert!(wait.contains("enable_and_hlt()"));
    assert!(!wait.contains("spin_loop()"));
    assert!(!wait.contains("block_current_for_io"));
    let inner = source.split("fn wait_timeout_inner(").nth(1).unwrap();
    assert!(
        inner.find("wait_idle_completion(self").unwrap()
            < inner.find("self.waiter.store(tid").unwrap()
    );
}
#[test]
fn kernel_schedule_admission_checks_the_count_before_exceptions() {
    let source = read("kernel/src/per_cpu.rs");
    let body = source.split("pub fn can_schedule(").nth(1).unwrap();
    assert!(
        body.find("if !returning_to_userspace && current_preempt & 0x0fff_ffff != 0")
            .unwrap()
            < body.find("in_exception_cleanup_context()").unwrap()
    );
}
#[test]
fn oracle_measures_switches_and_final_test_block_return() {
    let main = read("kernel/src/main.rs");
    assert!(
        main.find("disk_wait_oracle::begin()").unwrap()
            < main
                .find("\n        test_exec::test_direct_execution();")
                .unwrap()
    );
    assert!(
        main.find("test_exec::test_fbinfo()").unwrap()
            < main.find("disk_wait_oracle::tests_completed()").unwrap()
    );
    let switch = read("kernel/src/interrupts/context_switch.rs");
    assert!(
        switch.find("if old_thread_id == new_thread_id").unwrap()
            < switch.find("SWITCHED_AWAY.fetch_add").unwrap()
    );
    assert!(read("scripts/x86-gate-verdict.sh").contains("score-boot-disk-wait.py"));
}

#[test]
fn scorer_rejects_mutation_missing_duplicate_and_unfinished_evidence() {
    let status = std::process::Command::new("python3")
        .args([
            "-c",
            r#"
import importlib.util
from pathlib import Path
p = Path('scripts/score-boot-disk-wait.py')
spec = importlib.util.spec_from_file_location('oracle', p)
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
assert m.score(m.FINAL + m.PASS)
assert not m.score(m.PASS)
for bad in ['', m.PASS + m.PASS,
            m.PASS.replace('switched_away=0', 'switched_away=1'),
            m.PASS.replace('tests_completed=1', 'tests_completed=0'),
            m.PASS.replace(':PASS]', ':FAIL]'),
            m.PASS + m.PREFIX,
            m.PASS.replace('x86:', 'aarch64:')]:
    assert not m.score(m.FINAL + bad), bad
"#,
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .unwrap();
    assert!(status.success());
}
