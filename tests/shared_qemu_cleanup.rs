#[path = "shared_qemu/process_tree.rs"]
mod process_tree;

#[path = "shared_qemu/signal_handlers.rs"]
mod signal_handlers;

use process_tree::ChildGroup;
use signal_handlers::SignalHandlers;
use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn owned_runner_reaps_its_descendant_on_normal_and_unwinding_exit() {
    for panic_on_exit in [false, true] {
        let pid_file = std::env::temp_dir().join(format!(
            "breenix-shared-qemu-child-{}-{}",
            std::process::id(),
            panic_on_exit
        ));
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("sleep 30 & echo $! > \"$1\"; wait")
            .arg("sh")
            .arg(&pid_file);
        let mut runner = ChildGroup::spawn(&mut command).expect("spawn owned test runner");
        assert!(runner.try_wait().expect("check test runner").is_none());
        let pid = read_pid(&pid_file);
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
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "runner left child PID {pid} alive"
        );
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
        command
            .arg("-c")
            .arg("sleep 30 & echo $! > \"$1\"; wait")
            .arg("sh")
            .arg(std::env::var_os(PID_FILE).unwrap());
        let mut runner = ChildGroup::spawn(&mut command).expect("spawn signal test runner");
        while !SignalHandlers::interrupted() {
            assert!(runner
                .try_wait()
                .expect("check signal test runner")
                .is_none());
            thread::sleep(Duration::from_millis(20));
        }
        drop(runner);
        return;
    }

    let pid_file = std::env::temp_dir().join(format!(
        "breenix-shared-qemu-interrupted-child-{}",
        std::process::id()
    ));
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let mut helper = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "interrupted_runner_reaps_its_descendant"])
            .env(HELPER_FLAG, "1")
            .env(PID_FILE, &pid_file)
            .spawn()
            .expect("spawn signal helper");
        let pid = read_pid(&pid_file);
        // SAFETY: the PID is the helper process this test just spawned.
        assert_eq!(unsafe { libc::kill(helper.id() as libc::pid_t, signal) }, 0);
        assert_eq!(
            helper.wait().expect("reap signal helper").signal(),
            Some(signal)
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "SIGTERM left child PID {pid} alive"
        );
        fs::remove_file(&pid_file).expect("remove signal test PID file");
    }
}

fn read_pid(path: &std::path::Path) -> libc::pid_t {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(contents) = fs::read_to_string(path) {
            if let Ok(pid) = contents.trim().parse() {
                return pid;
            }
        }
        assert!(Instant::now() < deadline, "no complete child PID");
        thread::sleep(Duration::from_millis(20));
    }
}

#[path = "shared_qemu/boot_lock.rs"]
mod boot_lock;

#[test]
fn killed_owner_is_recovered_before_next_boot() {
    const HELPER: &str = "BREENIX_STALE_GROUP_HELPER";
    if let Some(path) = std::env::var_os(HELPER) {
        let _signals = SignalHandlers::install();
        let mut lock = boot_lock::BootLock::acquire(path.to_str().unwrap()).unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 30 & echo $! > \"$1\"; wait", "sh"])
            .arg(std::env::var_os("BREENIX_STALE_CHILD").unwrap());
        let _group = lock.spawn(&mut command).unwrap();
        // Publish readiness only after BootLock has saved the group identity.
        // The child's PID file can appear before spawn() finishes registration.
        fs::write(std::env::var_os("BREENIX_STALE_READY").unwrap(), b"registered\n").unwrap();
        loop {
            thread::sleep(Duration::from_millis(20));
        }
    }
    let root = std::env::temp_dir().join(format!("breenix-stale-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let lock_path = root.join("lock");
    let child_path = root.join("pid");
    let ready_path = root.join("ready");
    let mut helper = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "killed_owner_is_recovered_before_next_boot"])
        .env(HELPER, &lock_path)
        .env("BREENIX_STALE_CHILD", &child_path)
        .env("BREENIX_STALE_READY", &ready_path)
        .spawn()
        .unwrap();
    let pid = read_pid(&child_path);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !matches!(fs::read(&ready_path).as_deref(), Ok(b"registered\n")) {
        assert!(Instant::now() < deadline, "owner did not register its group");
        thread::sleep(Duration::from_millis(1));
    }
    helper.kill().unwrap();
    helper.wait().unwrap();
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        0,
        "fixture must leave a real stale group"
    );
    let lock = boot_lock::BootLock::acquire(lock_path.to_str().unwrap()).unwrap();
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "stale QEMU descendant survived recovery"
    );
    drop(lock);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn exited_leader_is_retained_until_descendants_are_killed() {
    let pid_file =
        std::env::temp_dir().join(format!("breenix-exited-leader-{}", std::process::id()));
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "sleep 30 & echo $! > \"$1\"; exit 7", "sh"])
        .arg(&pid_file);
    let mut group = ChildGroup::spawn(&mut command).unwrap();
    let pid = read_pid(&pid_file);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = group.try_wait().unwrap() {
            assert_eq!(status.code(), Some(7));
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        unsafe { libc::kill(group.id(), 0) },
        0,
        "leader PID was reaped too early"
    );
    drop(group);
    let deadline = Instant::now() + Duration::from_secs(3);
    while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    fs::remove_file(pid_file).unwrap();
}

#[test]
fn child_group_disconnects_terminal_input() {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "test ! -t 0 && test -c /dev/stdin"])
        .stdin(std::process::Stdio::piped());
    let mut group = ChildGroup::spawn(&mut command).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = group.try_wait().unwrap() {
            assert!(status.success(), "child stdin is not disconnected");
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
}
