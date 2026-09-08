use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn validate(source: &str) -> Result<(), &'static str> {
    for required in [
        "const CYCLES: u64 = 32;",
        "scheduler::yield_current();",
        "scheduler::schedule();",
        "mismatch += unsafe { witness_resume() };",
        "ACK.load(Ordering::Acquire) == cycle",
        "REQUEST.store(cycle, Ordering::Release);",
        "ACK.store(cycle, Ordering::Release);",
        "kthread::kthread_run(",
        "kthread::kthread_park_if(|| REQUEST.load(Ordering::Acquire) < cycle);",
        "kthread::kthread_unpark(&peer);",
        "cycles == CYCLES && mismatch == 0",
        "\"pushfq\",",
        "\"cmp qword ptr [rsp], 0x247\",",
        "\"cmp rax, r10\",",
        "\"cmp rax, r11\",",
    ] {
        if !source.contains(required) {
            return Err("missing resume witness or peer handshake");
        }
    }
    for (reg, value) in [
        ("rax", "567a"),
        ("rbx", "567b"),
        ("rbp", "567d"),
        ("r12", "5612"),
        ("r13", "5613"),
        ("r14", "5614"),
        ("r15", "5615"),
        ("rcx", "567c"),
        ("rdx", "567e"),
        ("rsi", "5675"),
        ("rdi", "5676"),
        ("r8", "5678"),
        ("r9", "5679"),
    ] {
        if !source.contains(&format!("\"mov {reg}, 0x{value}\""))
            || !source.contains(&format!("\"cmp {reg}, 0x{value}\",\n        \"jne 3f\""))
        {
            return Err("register witness lost initialization or failure branch");
        }
    }
    Ok(())
}

#[test]
fn witnesses_and_peer_are_required() {
    let source = fs::read_to_string(root().join("kernel/src/task/boot_resume_oracle.rs")).unwrap();
    validate(&source).unwrap();
    for needle in [
        "mismatch += unsafe { witness_resume() };",
        "kthread::kthread_unpark(&peer);",
        "scheduler::schedule();",
        "\"cmp rdx, 0x567e\"",
    ] {
        let mutated = source.replace(needle, "");
        assert_ne!(source, mutated);
        assert!(validate(&mutated).is_err());
    }
}

#[test]
fn scorer_rejects_missing_short_corrupt_duplicate_and_failed_results() {
    let script = r#"
import subprocess, sys, tempfile
from pathlib import Path
p = Path('scripts/score-boot-resume.py')
names = ('loopback_recv_wake_when_idle', 'loopback_recv_wake_under_load',
         'loopback_pump_does_not_busy_spin', 'tcp_final_ack_survives_accept_publish_race',
         'loopback_wake_loss_counters_are_zero')
good = '[BOOT_RESUME_ORACLE:x86:cycles=32:mismatch=0:PASS]\n'
for name in names:
    good += f'[TEST:network:{name}:START]\n[TEST:network:{name}:PASS]\n'
def run_cli(text):
    with tempfile.TemporaryDirectory() as directory:
        serial = Path(directory) / 'serial.txt'
        serial.write_text(text)
        return subprocess.run([sys.executable, str(p), str(serial)], capture_output=True).returncode
assert run_cli(good) == 0
variants = ['', good.replace('cycles=32', 'cycles=31'),
            good.replace('mismatch=0', 'mismatch=1'), good + good,
            good.replace('mismatch=0:PASS', 'mismatch=1:FAIL')]
for name in names:
    marker = f'[TEST:network:{name}:PASS]'
    variants += [good.replace(marker, ''), good.replace(marker, marker.replace('PASS', 'FAIL:injected'))]
for bad in variants:
    assert run_cli(bad) == 1, 'scorer CLI accepted mutation: ' + repr(bad)
"#;
    assert!(Command::new("python3")
        .arg("-c")
        .arg(script)
        .current_dir(root())
        .status()
        .unwrap()
        .success());
}
