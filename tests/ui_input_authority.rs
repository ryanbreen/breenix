//! A supervisor channel must reject writes after the sender drops root.
use std::{fs, process::Command};

fn function<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source.find(name).expect(name);
    let brace = start + source[start..].find('{').unwrap();
    let mut depth = 0;
    let end = source[brace..].bytes().position(|b| {
        if b == b'{' { depth += 1; }
        if b == b'}' { depth -= 1; }
        depth == 0
    }).unwrap();
    &source[start..=brace+end]
}

#[test]
fn root_control_checks_current_sender_before_queueing() {
    let source = include_str!("../kernel/src/socket/unix.rs");
    let policy = function(source, "fn authorize_sender(");
    let dir = std::env::temp_dir().join(format!("ui-input-authority-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("check.rs"), format!(r#"
const EPERM:i32=1;
{policy}
fn main() {{
 assert_eq!(authorize_sender(false,1000),Ok(()));
 assert_eq!(authorize_sender(true,0),Ok(()));
 assert_eq!(authorize_sender(true,1000),Err(EPERM));
 assert_eq!(authorize_sender(true,u32::MAX),Err(EPERM));
 assert_eq!(authorize_sender(true,0),Ok(()));
}}
"#)).unwrap();
    let build = Command::new("rustc").args(["--edition=2021", "-Dwarnings"])
        .arg(dir.join("check.rs")).arg("-o").arg(dir.join("check")).output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(dir.join("check")).output().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let send = function(source, "fn sendmsg(");
    assert!(send.find("authorize_sender(state.root_only, msg.cred.uid)?").unwrap()
        < send.find("state.rx.push").unwrap());
    let syscall = include_str!("../kernel/src/syscall/socket.rs");
    assert!(function(syscall,"fn send_cred(").contains("caller_ucred()"));
    let channel = include_str!("../libs/libuitest/src/supervisor_runtime.rs");
    assert!(channel.find("channel.require_root_sender()?").unwrap() < channel.find("Message::Ready(").unwrap());
}

#[test]
fn inherited_uinput_rechecks_effective_uid_at_write() {
    let source=include_str!("../kernel/src/input/stream.rs");
    let write=function(source,"fn write(&self,");
    assert!(write.find("Caller::current()").expect("current writer credentials")
        < write.find("self.device.lock()").unwrap());
    assert!(write.contains("if !caller.is_root()"));
    assert!(write.contains("return Err(EPERM as u64)"));
}
