mod shared_qemu;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn ready_kernel_line_reaps_the_cargo_runner_and_its_child() {
    if std::env::var_os("BREENIX_HARNESS_HELPER").is_none() {
        assert!(Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "ready_kernel_line_reaps_the_cargo_runner_and_its_child"
            ])
            .env("BREENIX_HARNESS_HELPER", "1")
            .status()
            .unwrap()
            .success());
        return;
    }
    let directory = std::env::temp_dir().join(format!(
        "breenix-shared-qemu-fixture-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).expect("create fixture directory");
    let fake_cargo = directory.join("cargo");
    let pid_file = directory.join("child.pid");
    fs::write(
        &fake_cargo,
        "#!/bin/sh\ncount=0; for arg in \"$@\"; do [ \"$arg\" != -serial ] || count=$((count + 1)); done\n[ \"$count\" = 2 ] || exit 9\nsleep 30 &\necho $! > \"$BREENIX_FAKE_QEMU_CHILD\"\nprintf '[ INFO] kernel: [CHECKPOINT:POST_COMPLETE]\\n' > target/shared_kernel_test_output.txt\nwait\n",
    )
    .expect("write fake cargo");
    fs::set_permissions(&fake_cargo, fs::Permissions::from_mode(0o755))
        .expect("make fake cargo executable");
    let original_path = std::env::var_os("PATH").expect("PATH");
    std::env::set_var(
        "PATH",
        format!(
            "{}:{}",
            directory.display(),
            original_path.to_string_lossy()
        ),
    );
    std::env::set_var("BREENIX_FAKE_QEMU_CHILD", &pid_file);
    let output = shared_qemu::get_kernel_output();
    std::env::set_var("PATH", original_path);
    std::env::remove_var("BREENIX_FAKE_QEMU_CHILD");
    assert!(output
        .lines()
        .any(|line| line.ends_with("kernel: [CHECKPOINT:POST_COMPLETE]")));
    let pid: libc::pid_t = fs::read_to_string(&pid_file)
        .expect("child PID")
        .trim()
        .parse()
        .expect("numeric child PID");
    let deadline = Instant::now() + Duration::from_secs(3);
    while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "shared QEMU left PID {pid} alive"
    );
    fs::remove_dir_all(directory).expect("remove fixture directory");
}

#[test]
fn failed_initialization_is_cached() {
    if std::env::var_os("BREENIX_FAILED_HARNESS_HELPER").is_none() {
        assert!(Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "failed_initialization_is_cached"])
            .env("BREENIX_FAILED_HARNESS_HELPER", "1")
            .status()
            .unwrap()
            .success());
        return;
    }
    let directory =
        std::env::temp_dir().join(format!("breenix-failed-harness-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let cargo = directory.join("cargo");
    let calls = directory.join("calls");
    fs::write(
        &cargo,
        "#!/bin/sh\necho invoked >> \"$BREENIX_FAKE_CALLS\"\nexit 7\n",
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var("PATH").unwrap();
    std::env::set_var("PATH", format!("{}:{path}", directory.display()));
    std::env::set_var("BREENIX_FAKE_CALLS", &calls);
    for _ in 0..2 {
        assert!(std::panic::catch_unwind(shared_qemu::get_kernel_output).is_err());
    }
    assert_eq!(fs::read_to_string(calls).unwrap().lines().count(), 1);
    fs::remove_dir_all(directory).unwrap();
}
