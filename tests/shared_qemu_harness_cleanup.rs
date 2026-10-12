mod shared_qemu;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn ready_kernel_line_reaps_the_cargo_runner_and_its_child() {
    let directory = std::env::temp_dir().join(format!(
        "breenix-shared-qemu-fixture-{}", std::process::id()
    ));
    fs::create_dir_all(&directory).expect("create fixture directory");
    let fake_cargo = directory.join("cargo");
    let pid_file = directory.join("child.pid");
    fs::write(
        &fake_cargo,
        "#!/bin/sh\nsleep 30 &\necho $! > \"$BREENIX_FAKE_QEMU_CHILD\"\nprintf '[ INFO] kernel: [CHECKPOINT:POST_COMPLETE]\\n' > target/shared_kernel_test_output.txt\nwait\n",
    )
    .expect("write fake cargo");
    fs::set_permissions(&fake_cargo, fs::Permissions::from_mode(0o755))
        .expect("make fake cargo executable");
    let original_path = std::env::var_os("PATH").expect("PATH");
    std::env::set_var("PATH", format!("{}:{}", directory.display(), original_path.to_string_lossy()));
    std::env::set_var("BREENIX_FAKE_QEMU_CHILD", &pid_file);
    let output = shared_qemu::get_kernel_output();
    std::env::set_var("PATH", original_path);
    std::env::remove_var("BREENIX_FAKE_QEMU_CHILD");
    assert!(output.lines().any(|line| line.ends_with("kernel: [CHECKPOINT:POST_COMPLETE]")));
    let pid: libc::pid_t = fs::read_to_string(&pid_file)
        .expect("child PID")
        .trim()
        .parse()
        .expect("numeric child PID");
    let deadline = Instant::now() + Duration::from_secs(3);
    while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "shared QEMU left PID {pid} alive");
    fs::remove_dir_all(directory).expect("remove fixture directory");
}
