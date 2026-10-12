#[path = "shared_qemu/process_tree.rs"]
mod process_tree;

#[path = "shared_qemu/signal_handlers.rs"]
mod signal_handlers;

use process_tree::ChildGroup;
use signal_handlers::SignalHandlers;
use std::fs;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn owned_runner_reaps_its_descendant_on_normal_and_unwinding_exit() {
    for panic_on_exit in [false, true] {
        let pid_file = std::env::temp_dir().join(format!(
            "breenix-shared-qemu-child-{}-{}",
            std::process::id(), panic_on_exit
        ));
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg("sleep 30 & echo $! > \"$1\"; wait").arg("sh").arg(&pid_file);
        let mut runner = ChildGroup::spawn(&mut command).expect("spawn owned test runner");
        assert!(runner.try_wait().expect("check test runner").is_none());
        let deadline = Instant::now() + Duration::from_secs(3);
        while !pid_file.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let pid: libc::pid_t = fs::read_to_string(&pid_file)
            .expect("child PID")
            .trim()
            .parse()
            .expect("numeric child PID");
        if panic_on_exit {
            let result = std::panic::catch_unwind(|| {
                let _owned = runner;
                panic!("simulated runner failure");
            });
            assert!(result.is_err());
        } else {
            drop(runner);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "runner left child PID {pid} alive");
        fs::remove_file(pid_file).expect("remove child PID file");
    }
}

#[test]
fn interrupted_runner_reaps_its_descendant() {
    const HELPER_FLAG: &str = "BREENIX_CLEANUP_SIGNAL_HELPER";
    const PID_FILE: &str = "BREENIX_CLEANUP_CHILD_PID_FILE";
    if std::env::var_os(HELPER_FLAG).is_some() {
        let _signals = SignalHandlers::install();
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg("sleep 30 & echo $! > \"$1\"; wait")
            .arg("sh").arg(std::env::var_os(PID_FILE).unwrap());
        let mut runner = ChildGroup::spawn(&mut command).expect("spawn signal test runner");
        while !SignalHandlers::interrupted() {
            assert!(runner.try_wait().expect("check signal test runner").is_none());
            thread::sleep(Duration::from_millis(20));
        }
        drop(runner);
        return;
    }

    let pid_file = std::env::temp_dir().join(format!(
        "breenix-shared-qemu-interrupted-child-{}", std::process::id()
    ));
    let mut helper = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", "interrupted_runner_reaps_its_descendant"])
        .env(HELPER_FLAG, "1")
        .env(PID_FILE, &pid_file)
        .spawn()
        .expect("spawn signal helper");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !pid_file.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    let pid: libc::pid_t = fs::read_to_string(&pid_file)
        .expect("signal test child PID")
        .trim()
        .parse()
        .expect("numeric signal test child PID");
    // SAFETY: the PID is the helper process this test just spawned.
    assert_eq!(unsafe { libc::kill(helper.id() as libc::pid_t, libc::SIGTERM) }, 0);
    assert!(helper.wait().expect("reap signal helper").success());
    let deadline = Instant::now() + Duration::from_secs(3);
    while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "SIGTERM left child PID {pid} alive");
    fs::remove_file(pid_file).expect("remove signal test PID file");
}
