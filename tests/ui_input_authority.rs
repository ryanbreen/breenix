//! A supervisor channel must reject writes after the sender drops root.
use std::{fs, process::Command};

fn function<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source.find(name).expect(name);
    let brace = start + source[start..].find('{').unwrap();
    let mut depth = 0;
    let end = source[brace..]
        .bytes()
        .position(|b| {
            if b == b'{' {
                depth += 1;
            }
            if b == b'}' {
                depth -= 1;
            }
            depth == 0
        })
        .unwrap();
    &source[start..=brace + end]
}

#[test]
fn root_control_checks_current_sender_before_queueing() {
    let source = include_str!("../kernel/src/socket/unix.rs");
    let policy = function(source, "fn authorize_sender(");
    let dir = std::env::temp_dir().join(format!("ui-input-authority-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("check.rs"),
        format!(
            r#"
const EPERM:i32=1;
{policy}
fn main() {{
 assert_eq!(authorize_sender(false,1000),Ok(()));
 assert_eq!(authorize_sender(true,0),Ok(()));
 assert_eq!(authorize_sender(true,1000),Err(EPERM));
 assert_eq!(authorize_sender(true,u32::MAX),Err(EPERM));
 assert_eq!(authorize_sender(true,0),Ok(()));
}}
"#
        ),
    )
    .unwrap();
    let build = Command::new("rustc")
        .args(["--edition=2021", "-Dwarnings"])
        .arg(dir.join("check.rs"))
        .arg("-o")
        .arg(dir.join("check"))
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(dir.join("check")).output().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let send = function(source, "fn sendmsg(");
    assert!(
        send.find("authorize_sender(state.root_only, msg.cred.uid)?")
            .unwrap()
            < send.find("state.rx.push").unwrap()
    );
    let syscall = include_str!("../kernel/src/syscall/socket.rs");
    assert!(function(syscall, "fn send_cred(").contains("caller_ucred()"));
    let channel = include_str!("../libs/libuitest/src/supervisor_runtime.rs");
    assert!(
        channel.find("channel.require_root_sender()?").unwrap()
            < channel.find("Message::Ready(").unwrap()
    );
}

#[test]
fn inherited_uinput_rechecks_effective_uid_at_write() {
    let source = include_str!("../kernel/src/input/stream.rs");
    let write = function(source, "fn write(&self,");
    assert!(
        write
            .find("Caller::current()")
            .expect("current writer credentials")
            < write.find("self.device.lock()").unwrap()
    );
    assert!(write.contains("if !caller.is_root()"));
    assert!(write.contains("return Err(EPERM as u64)"));
}

#[test]
fn root_policy_preserves_shutdown_flags() {
    let source=include_str!("../kernel/src/socket/object.rs");
    let constants=source.lines().filter(|line| line.starts_with("const SHUT_RD_BIT:")
        || line.starts_with("const SHUT_WR_BIT:") || line.starts_with("const ROOT_ONLY_BIT:")).collect::<Vec<_>>().join("\n");
    let methods=["fn set_shut(","fn shut_rd(","fn shut_wr(","fn root_only(","fn set_root_only("]
        .map(|name|function(source,name)).join("\n");
    let dir=std::env::temp_dir().join(format!("ui-root-flags-{}",std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("flags.rs"),format!(r#"
use std::sync::atomic::{{AtomicU8,Ordering}};
{constants}
struct State {{state_flags:AtomicU8}}
impl State {{{methods}}}
fn main() {{
 assert_eq!(ROOT_ONLY_BIT & (SHUT_RD_BIT|SHUT_WR_BIT),0);
 for how in 0..=2 {{
  let s=State {{state_flags:AtomicU8::new(0)}};
  assert!(!s.root_only()); s.set_root_only(true); assert!(s.root_only());
  s.set_shut(how);
  assert_eq!(s.shut_rd(),how==0 || how==2);
  assert_eq!(s.shut_wr(),how==1 || how==2);
  assert!(s.root_only()); s.set_root_only(false); assert!(!s.root_only());
  assert_eq!(s.shut_rd(),how==0 || how==2);
  assert_eq!(s.shut_wr(),how==1 || how==2);
  s.set_root_only(true); assert!(s.root_only());
 }}
}}
"#)).unwrap();
    let build=Command::new("rustc").args(["--edition=2021","-Dwarnings"])
        .arg(dir.join("flags.rs")).arg("-o").arg(dir.join("flags")).output().unwrap();
    assert!(build.status.success(),"{}",String::from_utf8_lossy(&build.stderr));
    let run=Command::new(dir.join("flags")).output().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(run.status.success(),"{}",String::from_utf8_lossy(&run.stderr));
}
