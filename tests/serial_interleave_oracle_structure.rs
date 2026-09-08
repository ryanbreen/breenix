use std::{fs, path::PathBuf, process::Command};
#[test]
fn scorer_requires_complete_uncorrupted_pair_of_streams() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let result = Command::new("python3").current_dir(&root).args(["-c", r#"
import importlib.util
spec=importlib.util.spec_from_file_location('s','scripts/score-serial-interleave.py')
s=importlib.util.module_from_spec(spec); spec.loader.exec_module(s)
rows=[f'[SERIAL_INTERLEAVE:cpu={cpu}:seq={n}:payload={chr(65+cpu)*768}]\n'.encode() for cpu in (1,2) for n in range(200)]
good=b''.join(rows)
assert s.score(good)==(True,400,0)
assert not s.score(b''.join(rows[1:]))[0]
assert not s.score(good+rows[0])[0]
assert not s.score(good.replace(b'payload=B',b'payload=C',1))[0]
spliced=rows[0][:100]+rows[200][:100]+rows[0][100:]+rows[200][100:]
assert s.score(spliced+b''.join(rows[1:200])+b''.join(rows[201:]))[2]>0
assert s.score(good+b'BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB\n')[2]>0
"#]).status().unwrap();
    assert!(result.success());
}
#[test]
fn strict_gate_and_boot_registry_require_the_oracle() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let gate =
        fs::read_to_string(root.join("docker/qemu/run-aarch64-boot-test-strict.sh")).unwrap();
    let scorer = gate
        .split("score_serial() {")
        .nth(1)
        .unwrap()
        .split("\n}")
        .next()
        .unwrap();
    assert!(scorer.contains("if ! python3 \"$BREENIX_ROOT/scripts/score-serial-interleave.py\" \"$serial_file\"; then\n        return 1"));
    let registry = fs::read_to_string(root.join("kernel/src/test_framework/registry.rs")).unwrap();
    assert!(registry.contains("func: crate::serial_line_oracle::run"));
    let oracle = fs::read_to_string(root.join("kernel/src/serial_line_oracle.rs")).unwrap();
    for required in [
        "kthread_run_on_cpu_for_test",
        "preempt_disable",
        "for seq in 0..200",
        "for _ in 0..768",
        "ROUND[1 - index].load(Ordering::Acquire)",
        "kthread_join",
    ] {
        assert!(oracle.contains(required), "missing {required}");
    }
}
