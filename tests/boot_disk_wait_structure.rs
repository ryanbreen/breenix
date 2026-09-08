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

#[test]
fn departure_counter_observes_dispatch_result_after_rollback() {
    let source = read("kernel/src/interrupts/context_switch.rs");
    let caller = source
        .split("pub extern \"C\" fn check_need_resched_and_switch(")
        .nth(1)
        .unwrap();
    let call = caller.find("        switch_to_thread(").unwrap();
    let count = caller.find("SWITCHED_AWAY.fetch_add").unwrap();
    assert!(
        call < count,
        "selected dispatch is not a completed departure"
    );
    let observation = &caller[call..count];
    assert!(observation.contains("current_thread_id_lock_free()"));
    assert!(observation.contains("dispatched_tid != old_thread_id"));
    assert!(observation.contains("BOOT_TID.load(Ordering::Acquire) == old_thread_id"));
}

#[test]
fn switch_and_admission_have_no_text_output() {
    let switch = read("kernel/src/interrupts/context_switch.rs");
    let per_cpu = read("kernel/src/per_cpu.rs");
    let admission = per_cpu
        .split("pub fn can_schedule(")
        .nth(1)
        .unwrap()
        .split("/// Get per-CPU base address")
        .next()
        .unwrap();
    for source in [&switch[..], admission] {
        for line in source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
        {
            for forbidden in [
                "log::trace!",
                "log::debug!",
                "log::info!",
                "log::warn!",
                "log::error!",
                "raw_serial_",
                "serial_print",
                "Port::new",
                "port.write(",
            ] {
                assert!(
                    !line.contains(forbidden),
                    "text output in switch/admission: {line}"
                );
            }
        }
    }
}

#[test]
fn resolved_departure_block_rejects_tls_rollback_and_counts_departure() {
    let source = read("kernel/src/interrupts/context_switch.rs");
    let observation = source
        .split("// Observe the resolved dispatch,")
        .nth(1)
        .unwrap();
    let block = observation.split("        // #772:").next().unwrap();
    let block = &block[block.find("        #[cfg").unwrap()..];
    let harness = format!(
        r#"
use std::sync::atomic::{{AtomicU64, Ordering}};
mod boot {{ pub mod disk_wait_oracle {{
    use std::sync::atomic::AtomicU64;
    pub static BOOT_TID: AtomicU64 = AtomicU64::new(42);
    pub static SWITCHED_AWAY: AtomicU64 = AtomicU64::new(0);
    pub static SWITCH_PREEMPT: AtomicU64 = AtomicU64::new(u64::MAX);
}} }}
static CURRENT: AtomicU64 = AtomicU64::new(42);
mod per_cpu {{
    pub fn current_thread_id_lock_free() -> Option<u64> {{
        match super::CURRENT.load(super::Ordering::Relaxed) {{ 0 => None, id => Some(id) }}
    }}
}}
fn observe(old_thread_id: u64, outgoing_preempt: u64) {{ {block} }}
fn main() {{
    use boot::disk_wait_oracle::*;
    // TLS rollback restores boot's identity before observation.
    observe(42, 7);
    assert_eq!(SWITCHED_AWAY.load(Ordering::Acquire), 0);
    assert_eq!(SWITCH_PREEMPT.load(Ordering::Relaxed), u64::MAX);
    CURRENT.store(43, Ordering::Relaxed);
    observe(42, 7);
    assert_eq!(SWITCHED_AWAY.load(Ordering::Acquire), 1);
    assert_eq!(SWITCH_PREEMPT.load(Ordering::Relaxed), 7);
    observe(43, 9);
    CURRENT.store(0, Ordering::Relaxed);
    observe(42, 9);
    CURRENT.store(42, Ordering::Relaxed);
    observe(42, 9);
    assert_eq!(SWITCHED_AWAY.load(Ordering::Acquire), 1);
    assert_eq!(SWITCH_PREEMPT.load(Ordering::Relaxed), 7);
}}
"#
    );
    let dir = std::env::temp_dir().join(format!("boot-departure-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("departure.rs");
    let binary = dir.join("departure");
    fs::write(&input, harness).unwrap();
    let compile = std::process::Command::new("rustc")
        .args(["--edition=2021", "--cfg", "feature=\"testing\""])
        .arg(&input)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    assert!(
        compile.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = std::process::Command::new(&binary).status().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(run.success());
}
