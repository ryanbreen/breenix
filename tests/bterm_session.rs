#[path = "../userspace/programs/src/bterm/session.rs"]
mod session;

#[test]
fn closing_a_tab_hangs_up_its_group_and_reaps_the_shell() {
    use std::{io::Read, os::unix::process::CommandExt, process::{Command, Stdio}};
    unsafe extern "C" { fn setsid() -> i32; fn kill(pid: i32, signal: i32) -> i32; }
    let mut command = Command::new("/bin/sh");
    // Both shells keep the setsid group; each records HUP through its exit status.
    command.args(["-c", r#"exec 3<&0; trap 'wait "$peer"; result=$?; if [ "$result" = 74 ]; then exit 73; else exit 75; fi' HUP; /bin/sh -c 'trap "exit 74" HUP; printf ready; read line' <&3 & peer=$!; wait "$peer"; read line"#])
        .stdin(Stdio::piped()).stdout(Stdio::piped());
    unsafe { command.pre_exec(|| if setsid() < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }); }
    let mut child = command.spawn().unwrap();
    let pid = i32::try_from(child.id()).unwrap();
    let mut ready = [0; 5];
    child.stdout.as_mut().unwrap().read_exact(&mut ready).unwrap();
    assert_eq!(&ready, b"ready");
    use std::os::fd::IntoRawFd;
    let master = std::fs::File::open("/dev/null").unwrap().into_raw_fd();
    let status = session::hangup_and_reap(pid, Some(pid), master).unwrap().unwrap();
    assert_eq!(status >> 8, 73, "shell did not receive SIGHUP");
    assert_eq!(unsafe { kill(-pid, 0) }, -1, "tab group survived hangup");
    assert_eq!(child.try_wait().unwrap_err().raw_os_error(), Some(10), "shell was not reaped");
}
