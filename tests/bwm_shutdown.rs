#[path = "../userspace/programs/src/bwm/shutdown.rs"]
mod shutdown;

use shutdown::{boundary, Runtime};

#[derive(Default)]
struct Fake {
    term: bool,
    pending: bool,
    blocked: bool,
    term_in_wait: bool,
    events: Vec<&'static str>,
}

impl Runtime for Fake {
    type Ready = ();
    fn unblock(&mut self) { self.events.push("unblock"); self.blocked = false; if self.pending { self.term = true; } }
    fn wait(&mut self) { self.events.push("wait"); if self.term_in_wait { self.term = true; } }
    fn block(&mut self) { self.events.push("block"); self.blocked = true; }
    fn terminated(&self) -> bool { self.term }
    fn release(&mut self) { self.events.push("release"); }
    fn restore(&mut self) { self.events.push("restore"); }
    fn exit(&mut self, status: i32) { assert_eq!(status, 0); self.events.push("exit:0"); }
}

#[test]
fn term_during_gpu_work_waits_for_boundary_and_returns_custody() {
    let mut fake = Fake { blocked: true, ..Fake::default() };
    fake.events.push("gpu-start");
    fake.pending = true;
    assert!(fake.blocked);
    fake.events.push("gpu-end");
    assert!(!fake.term);
    assert!(boundary(&mut fake).is_none());
    assert_eq!(fake.events, ["gpu-start", "gpu-end", "unblock", "block", "release", "restore", "exit:0"]);
}

#[test]
fn term_interrupting_poll_returns_custody_before_any_more_gpu_work() {
    let mut fake = Fake { blocked: true, term_in_wait: true, ..Fake::default() };
    assert!(boundary(&mut fake).is_none());
    assert!(fake.blocked);
    assert_eq!(fake.events, ["unblock", "wait", "block", "release", "restore", "exit:0"]);
}

#[test]
fn ordinary_poll_reblocks_term_without_releasing_custody() {
    let mut fake = Fake { blocked: true, ..Fake::default() };
    assert_eq!(boundary(&mut fake), Some(()));
    assert!(fake.blocked);
    assert_eq!(fake.events, ["unblock", "wait", "block"]);
}

#[test]
fn direct_spawn_starts_with_fresh_signal_state() {
    // The shared direct-spawn implementation creates a new Process, whose
    // default SignalState supplies both an empty mask and default handlers.
    let manager = include_str!("../kernel/src/process/manager.rs");
    let process = include_str!("../kernel/src/process/process.rs");
    let signals = include_str!("../kernel/src/signal/types.rs");
    assert!(manager.contains("let mut process = Process::new(pid, name.clone(), loaded_elf.entry_point)"));
    assert!(manager.contains("let mut process = Process::new(pid, name.clone(), entry_point)"));
    assert!(process.contains("signals: SignalState::default()"));
    assert!(signals.contains("thread: Arc::new(ThreadSignals::with_mask(0))"));
    assert!(signals.contains("SignalTable::new()"));
}


// Execute the hotkey, libbreenix wrapper and both kernel spawn bodies. Only
// allocation/loading and unrelated process attributes are replaced by the fake.
#[test]
fn hotkey_spawn_delivers_term_with_default_action_on_both_spawn_paths() {
    use std::{fs, process::Command};
    let root = std::path::Path::new(file!()).canonicalize().unwrap()
        .parent().unwrap().parent().unwrap().to_path_buf();
    fn item(source: &str, marker: &str) -> String {
        let start = source.find(marker).unwrap();
        let open = start + source[start..].find('{').unwrap();
        let mut depth = 0;
        for (offset, byte) in source[open..].bytes().enumerate() {
            match byte { b'{' => depth += 1, b'}' => depth -= 1, _ => {} }
            if depth == 0 { return source[start..=open + offset].to_string(); }
        }
        panic!("unterminated production item: {marker}");
    }
    let manager = fs::read_to_string(root.join("kernel/src/process/manager.rs")).unwrap();
    let first = manager.find("pub fn spawn_process(").unwrap();
    let second = first + 1 + manager[first + 1..].find("pub fn spawn_process(").unwrap();
    let process = fs::read_to_string(root.join("kernel/src/process/process.rs")).unwrap();
    let constructor = item(&process, "pub fn new(id: ProcessId,");
    let initializer = constructor.split("signals:").nth(1).unwrap().split(',').next().unwrap();
    let wrapper = fs::read_to_string(root.join("libs/libbreenix/src/process.rs")).unwrap();
    let launch = fs::read_to_string(root.join("userspace/programs/src/bwm/launch.rs")).unwrap();
    let fixture = include_str!("fixtures/bwm_spawn.rs")
        .replace("@ROOT@", root.to_str().unwrap())
        .replace("@SIGNALS@", initializer)
        .replace("@SPAWN_X86@", &item(&manager[first..], "pub fn spawn_process("))
        .replace("@SPAWN_ARM@", &item(&manager[second..], "pub fn spawn_process(").replace("fn spawn_process(", "fn spawn_arm("))
        .replace("@WRAPPER@", &item(&wrapper, "pub fn spawn(path:"))
        .replace("@HOTKEY@", &item(&launch, "pub(super) fn execute_hotkey_action").replace("pub(super)", ""));
    let dir = std::env::temp_dir().join(format!("bwm-spawn-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("spawn.rs");
    let binary = dir.join("spawn-check");
    fs::write(&source, fixture).unwrap();
    let build = Command::new("rustc").args(["--edition=2021", "--test", "-Dwarnings"])
        .arg(&source).arg("-o").arg(&binary).output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&binary).output().unwrap();
    assert!(run.status.success(), "{}{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn child_signal_interrupts_wait_without_shutdown() {
    struct ChildWait { interrupted: bool, terminated: bool }
    impl Runtime for ChildWait {
        type Ready = bool;
        fn unblock(&mut self) {}
        fn wait(&mut self) -> bool { self.interrupted = true; true }
        fn block(&mut self) {}
        fn terminated(&self) -> bool { self.terminated }
        fn release(&mut self) { panic!("child exit must not release display"); }
        fn restore(&mut self) { panic!("child exit must not restore display"); }
        fn exit(&mut self, _: i32) { panic!("child exit must not terminate bwm"); }
    }
    let mut wait = ChildWait { interrupted: false, terminated: false };
    assert_eq!(boundary(&mut wait), Some(true));
    assert!(wait.interrupted);
    let source = include_str!("../userspace/programs/src/bwm.rs");
    assert!(source.contains("signal::sigaction(signal::SIGCHLD"));
    assert!(source.contains("signal::SIGCHLD - 1"));
}

#[test]
fn child_exit_before_poll_leaves_a_readable_wakeup() {
    use std::{fs, process::Command};
    let main = include_str!("../userspace/programs/src/bwm.rs");
    let start = main.find("extern \"C\" fn child_exited(").unwrap();
    let open = start + main[start..].find('{').unwrap();
    let mut depth = 1;
    let mut end = open + 1;
    while depth != 0 {
        match main.as_bytes()[end] { b'{' => depth += 1, b'}' => depth -= 1, _ => {} }
        end += 1;
    }
    let fixture = include_str!("fixtures/bwm_signal_wait.rs")
        .replace("@HANDLER@", &main[start..end]);
    let dir = std::env::temp_dir().join(format!("bwm-signal-wait-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("wait.rs");
    let binary = dir.join("wait-check");
    fs::write(&source, fixture).unwrap();
    let build = Command::new("rustc").args(["--edition=2021", "--test", "-Dwarnings"])
        .arg(&source).arg("-o").arg(&binary).output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&binary).output().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(run.status.success(), "{}{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
}
