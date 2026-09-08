//! Deferral ownership and self-test evidence for issue 891.
use std::{fs, path::PathBuf};
fn read(path: &str) -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}
#[test]
fn daemon_is_cpu_local_and_checks_pending_after_park_publication() {
    let core = read("kernel/src/task/softirqd.rs");
    assert!(core.contains("kthread_run_on_cpu(ksoftirqd_fn"));
    assert!(core.contains("KSOFTIRQD.get(current_cpu())"));
    assert!(core.contains("kthread_park_if(|| per_cpu::softirq_pending() == 0)"));
    let kthread = read("kernel/src/task/kthread.rs");
    let park = kthread.split("pub fn kthread_park_if").nth(1).unwrap();
    assert!(park.find("parked.store(true").unwrap() < park.find("!should_park()").unwrap());
}
#[test]
fn oracle_requires_daemon_dispatch_and_guest_ticks() {
    let source = read("kernel/src/task/softirq_tests.rs");
    assert!(!source.contains("for _ in 0..100"));
    assert!(source.contains("dispatches > 0 && iterations >= TARGET_ITERATIONS"));
    assert!(source.contains("elapsed_ticks >= WAIT_TICKS"));
    assert!(source.contains("if !GUEST_DONE.load(Ordering::Acquire)"));
    assert!(source.contains("SOFTIRQ_DEFERRAL_ORACLE:"));
    assert!(source.contains("\"lost\""));
    assert!(source.contains("\"starved\""));
    assert!(!source.contains("ksoftirqd should have processed deferred softirqs"));
}
#[test]
fn scorers_require_oracle() {
    for path in ["docker/qemu/run-aarch64-boot-test-strict.sh", "docker/qemu/run-x86-boot-tests.sh", "scripts/x86-gate-verdict.sh"] {
        assert!(read(path).contains("score-softirq-deferral.py"), "{path}");
    }
}
#[test]
fn limit_wakes_daemon_and_counter_is_at_callback() {
    let source = read("kernel/src/task/softirqd.rs");
    let limit = source.split("if restart_count >= MAX_SOFTIRQ_RESTART").nth(1).unwrap().split("return true;").next().unwrap();
    assert!(limit.contains("wakeup_ksoftirqd();"));
    let daemon = source.split("fn ksoftirqd_fn()").nth(1).unwrap().split("pub fn init_softirq").next().unwrap();
    assert!(daemon.contains("KSOFTIRQD_TASKLET_DISPATCHES.increment();"));
    assert!(daemon.find("KSOFTIRQD_TASKLET_DISPATCHES.increment();").unwrap() < daemon.find("handler(softirq_type);").unwrap());
}
#[test]
fn scorer_rejects_forged_ok_and_reports_starvation_separately() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let result = std::process::Command::new("python3").current_dir(root)
        .args(["-c", r#"
import importlib.util
spec=importlib.util.spec_from_file_location('score','scripts/score-softirq-deferral.py')
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
def line(ticks=2,dispatches=5,iterations=25,verdict='ok',ns=15000000000):
    return f'[SOFTIRQ_DEFERRAL_ORACLE:arch=x86:cpu=0:budget_ticks=250:wait_ticks={ticks}:wait_ns={ns}:dispatches={dispatches}:iterations={iterations}:verdict={verdict}]'
assert m.score(line())[0] == 0
assert m.score(line(dispatches=0))[0] == 1
assert m.score(line(iterations=24))[0] == 1
assert m.score(line(verdict='lost'))[0] == 1
assert m.score(line(ticks=2,dispatches=0,verdict='starved'))[0] == 2
for ns in (0, 123, 14999999999):
    assert m.score(line(dispatches=0,iterations=10,verdict='starved',ns=ns))[0] == 1
assert m.score(line(dispatches=0,iterations=10,verdict='starved',ns=15000000001))[0] == 2
assert m.score(line(ticks=250,dispatches=0,verdict='starved'))[0] == 1
assert m.score('')[0] == 1
assert m.score(line()+'\n'+line())[0] == 1
"#]).status().unwrap();
    assert!(result.success());
}
#[test]
fn park_final_check_and_unpark_flag_share_scheduler_serialization() {
    let source = read("kernel/src/task/kthread.rs");
    let park = source.split("pub fn kthread_park_if").nth(1).unwrap().split("pub fn kthread_unpark").next().unwrap();
    let block = park.split("scheduler::with_scheduler(|sched| {").nth(1).unwrap();
    assert!(block.find("parked.load(Ordering::Acquire)").unwrap() < block.find("sched.block_current()").unwrap());
    let wake = source.split("pub fn kthread_unpark").nth(1).unwrap().split("/// Test-only").next().unwrap();
    assert!(wake.find("scheduler::with_scheduler(|sched| {").unwrap() < wake.find("parked.store(false").unwrap());
}
#[test]
fn boot_cpu_daemon_is_published_only_after_boot_preemption_pin_is_released() {
    let core = read("kernel/src/task/softirqd.rs");
    assert!(core.contains("cpu == 0 && crate::per_cpu_aarch64::preempt_count() != 0"));
    let main = read("kernel/src/main_aarch64.rs");
    let handoff = main.split("fn launch_init_from_elf").nth(1).unwrap().split("pub extern \"C\" fn kernel_main").next().unwrap();
    assert!(handoff.find("preempt_enable();").unwrap() < handoff.find("init_online_daemons();").unwrap());
}

#[test]
fn strict_gate_preserves_starvation_status() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert!(std::process::Command::new("python3").current_dir(root)
        .arg("scripts/test-softirq-strict-status.py").status().unwrap().success());
}

#[test]
fn softirq_reexport_is_rustfmt_clean() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert!(std::process::Command::new("rustfmt").current_dir(root)
        .args(["--check", "--edition", "2021", "--config", "skip_children=true", "kernel/src/task/mod.rs"])
        .status().unwrap().success());
}
