//! Host checks of the production reaper state, lifecycle constructors and ancestor choice.
//! Run with `rustc --edition=2021 --test tests/subreaper_reparent.rs -o /tmp/subreaper-tests`.

use std::{fs, path::PathBuf, process::Command};

const PROCESS: &str = include_str!("../kernel/src/process/process.rs");
const MANAGER: &str = include_str!("../kernel/src/process/manager.rs");
const CLONE: &str = include_str!("../kernel/src/syscall/clone.rs");

fn item<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source.find(signature).unwrap_or_else(|| panic!("missing {signature}"));
    let brace = start + source[start..].find('{').unwrap();
    let mut depth = 0;
    for (offset, byte) in source[brace..].bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return &source[start..=brace + offset];
        }
    }
    panic!("unclosed {signature}");
}

fn process_new_count(source: &str) -> usize {
    source.match_indices("Process::new(").filter(|(offset, _)| {
        !source[..*offset].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_')
    }).count()
}

fn run_production(source: String) {
    let dir = std::env::temp_dir().join(format!("breenix-subreaper-{}-{:?}", std::process::id(), std::thread::current().id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cases.rs");
    fs::write(&path, source).unwrap();
    let result = Command::new("rustc")
        .args(["--edition=2021", "-Dwarnings", "--test"])
        .arg(&path).arg("-o").arg(dir.join("cases"))
        .output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let result = Command::new(dir.join("cases")).output().unwrap();
    assert!(result.status.success(), "{}{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn production_prctl_requests() {
    let root = std::env::current_dir().unwrap();
    let path = root.join("kernel/src/syscall/prctl.rs");
    assert!(path.exists(), "missing shared prctl handler");
    let state = [item(PROCESS, "pub struct ReaperState"), item(PROCESS, "impl ReaperState")].join("\n");
    run_production(format!(r#"
extern crate alloc;
{state}
pub mod task {{ pub mod scheduler {{
    pub fn current_thread_id() -> Option<u64> {{ Some(9) }}
}} }}
pub mod process {{
    pub struct Row {{ pub reaper: super::ReaperState }}
    pub struct Manager {{ pub row: Row }}
    impl Manager {{
        pub fn find_process_by_thread(&self, tid: u64) -> Option<(u64, &Row)> {{
            (tid == 9).then_some((10, &self.row))
        }}
    }}
    pub fn manager() -> std::sync::MutexGuard<'static, Option<Manager>> {{
        static MANAGER: std::sync::Mutex<Option<Manager>> = std::sync::Mutex::new(None);
        MANAGER.lock().unwrap()
    }}
}}
pub mod syscall {{
    #[derive(Debug, PartialEq)]
    pub enum SyscallResult {{ Ok(u64), Err(u64) }}
    pub mod errno {{ pub const ENOSYS: i32 = 38; pub const ESRCH: i32 = 3; }}
    pub mod userptr {{
        pub fn copy_to_user<T: Copy>(ptr: *mut T, value: &T) -> Result<(), u64> {{
            if (ptr as usize) < 4096 {{ return Err(14); }}
            unsafe {{ ptr.write_unaligned(*value); }}
            Ok(())
        }}
    }}
    #[path = {path:?}]
    pub mod prctl;
}}
#[test]
fn requests() {{
    use syscall::{{prctl::sys_prctl, SyscallResult::{{Ok, Err}}}};
    *process::manager() = Some(process::Manager {{ row: process::Row {{ reaper: ReaperState::new_process() }} }});
    let mut value = [99i32, 123i32];
    assert_eq!(sys_prctl(0x42520008, value.as_mut_ptr() as u64), Ok(0));
    assert_eq!(value, [0, 123]);
    for option in [0, 3, 36, 37, 0x42520001, 0x42520009, u64::MAX] {{
        assert_eq!(sys_prctl(option, 0), Err(38));
    }}
    assert_eq!(sys_prctl(0x42520007, 2), Ok(0));
    assert_eq!(sys_prctl(0x42520008, value.as_mut_ptr() as u64), Ok(0));
    assert_eq!(value, [1, 123]);
    assert_eq!(sys_prctl(0x42520008, 0), Err(14));
    assert_eq!(sys_prctl(0x42520007, 0), Ok(0));
    assert_eq!(sys_prctl(0x42520008, value.as_mut_ptr() as u64), Ok(0));
    assert_eq!(value, [0, 123]);
    // Exercise the other state constructors here too, with warnings denied.
    let state = ReaperState::new_process();
    assert!(!state.fork_child().is_subreaper());
    assert!(!state.share_thread().is_subreaper());
    *process::manager() = None;
    assert_eq!(sys_prctl(0x42520007, 1), Err(3));
}}
"#));
}

#[test]
fn production_constructors_exec_and_ancestor_selection() {
    let process_impl = &PROCESS[PROCESS.find("impl Process {").unwrap()..];
    let pieces = [
        item(PROCESS, "pub struct ReaperState"),
        item(PROCESS, "impl ReaperState"),
        item(PROCESS, "pub fn select_child_reaper"),
    ].join("\n");
    let new = item(process_impl, "pub fn new(");
    let allocation = item(new, "Process {");
    let fields = allocation.lines().filter(|line| {
        ["reaper:", "forked_without_exec:", "has_exec:"].iter()
            .any(|field| line.trim_start().starts_with(field))
    }).collect::<Vec<_>>().join("\n");
    assert_eq!(fields.lines().count(), 3);
    // Project only unrelated row fields out of the literal. Keep the production
    // reaper and image-lifetime initializers, fork constructor and exec method.
    let new = new.replace(allocation, &format!("Process {{\n{fields}\n}}"))
        .replacen('{', "{ let _ = (id, name, entry_point);", 1);
    let methods = [new, item(process_impl, "pub fn fork_child(").to_owned(),
        item(process_impl, "pub fn note_exec(").to_owned()].join("\n");
    run_production(format!(r#"
extern crate alloc;
type ProcessId = u64;
type String = alloc::string::String;
type VirtAddr = u64;
{pieces}
struct Process {{ reaper: ReaperState, forked_without_exec: bool, has_exec: bool }}
impl Process {{
    {methods}
}}
#[test]
fn lifecycle() {{
    let mut parent = Process::new(10, String::new(), 0);
    assert!(!parent.reaper.is_subreaper());
    parent.reaper.set_subreaper(true);
    let child = Process::fork_child(11, String::new(), 0, &parent);
    assert!(child.forked_without_exec);
    assert!(!child.reaper.is_subreaper());
    assert!(parent.reaper.is_subreaper());
    parent.note_exec();
    assert!(parent.has_exec);
    assert!(!parent.forked_without_exec);
    assert!(parent.reaper.is_subreaper());
    let thread = parent.reaper.share_thread();
    thread.set_subreaper(false);
    assert!(!parent.reaper.is_subreaper());
    assert!(!Process::new(12, String::new(), 0).reaper.is_subreaper());
}}
#[test]
fn nearest_live_ancestor_or_init() {{
    // id -> (parent, live, subreaper), nearest first: 30 -> 20 -> 10 -> 1.
    let mut rows = std::collections::BTreeMap::from([
        (30, (Some(20), true, false)),
        (20, (Some(10), true, false)),
        (10, (Some(1), true, false)),
        (1, (None, true, false)),
    ]);
    let choose = |rows: &std::collections::BTreeMap<u64, (Option<u64>, bool, bool)>|
        select_child_reaper(Some(20), Some(1), rows.len(), |pid| rows.get(&pid).copied());
    assert_eq!(choose(&rows), Some(1));
    rows.get_mut(&10).unwrap().2 = true;
    assert_eq!(choose(&rows), Some(10));
    rows.get_mut(&20).unwrap().2 = true;
    assert_eq!(choose(&rows), Some(20));
    rows.get_mut(&20).unwrap().1 = false;
    assert_eq!(choose(&rows), Some(10));
    rows.get_mut(&10).unwrap().1 = false;
    assert_eq!(choose(&rows), Some(1));
    assert_eq!(select_child_reaper(None, None, 0, |_| None), None);
    assert_eq!(select_child_reaper(Some(99), Some(1), 1, |_| None), Some(1));
    assert_eq!(select_child_reaper(Some(20), None, 1, |_| Some((None, true, true))), Some(20));
    assert_eq!(select_child_reaper(Some(20), Some(1), 4, |_| Some((Some(20), false, true))), Some(1));
}}
"#));
}

#[test]
fn fork_paths_use_the_child_constructor_and_exec_keeps_state() {
    for name in ["fork_process_with_page_table", "begin_fork", "fork_process_with_context"] {
        let body = item(MANAGER, &format!("fn {name}("));
        assert!(body.contains("Process::fork_child("), "{name} bypasses fork-child state");
        assert!(!body.contains("Process::new("), "{name} uses a new-process constructor");
    }
    let arm = item(MANAGER, "fn fork_process_aarch64(");
    assert!(arm.contains("self.begin_fork("));
    assert!(!arm.contains("Process::new("));
    assert!(CLONE.contains("Process::fork_child("));
    assert!(CLONE.contains("child_process.reaper = parent.reaper.share_thread()"));
    // Inventory constructor sites, including future fork/clone functions.
    assert_eq!(process_new_count(MANAGER), 4, "new fork path bypassed the constructor");
    assert_eq!(process_new_count(CLONE), 0);
    assert_eq!(MANAGER.matches("process.note_exec()").count(), 4);
    for body in MANAGER.split("fn exec_process").skip(1) {
        let body = item(&format!("fn exec_process{body}"), "fn exec_process").to_owned();
        assert!(body.contains("process.note_exec()"));
        assert!(!body.contains(".reaper ="));
        assert!(!body.contains("Process::new("));
    }
    let reparent = item(MANAGER, "fn reparent_children_to_init(");
    assert!(reparent.contains("select_child_reaper("));
    assert!(reparent.contains("!row.is_terminated() && !row.exit_started"));
    assert!(reparent.contains("row.reaper.is_subreaper()"));
    assert!(reparent.contains("process.parent = Some(reaper)"));
    assert!(reparent.contains("live_row_mut(&reaper)"));
}

#[test]
fn shared_dispatch_and_public_request_ids() {
    let root = PathBuf::from(file!()).parent().unwrap().parent().unwrap().to_owned();
    let dispatch = fs::read_to_string(root.join("kernel/src/syscall/mod.rs")).unwrap();
    assert!(dispatch.contains("pub const PRCTL_SYSCALL_NUMBER: u64 = 157;"));
    assert!(dispatch.contains("pub const PRCTL_SYSCALL_NUMBER: u64 = 167;"));
    assert!(item(&dispatch, "pub fn dispatch_numbered(").contains("PRCTL_SYSCALL_NUMBER => prctl::sys_prctl(a[0], a[1])"));
    let prctl = fs::read_to_string(root.join("kernel/src/syscall/prctl.rs")).unwrap();
    assert!(prctl.contains("0x42520007"));
    assert!(prctl.contains("0x42520008"));
    assert!(prctl.contains("ENOSYS"));
    let wrappers = fs::read_to_string(root.join("libs/libbreenix/src/process.rs")).unwrap();
    assert!(wrappers.contains("pub fn set_child_subreaper("));
    assert!(wrappers.contains("pub fn get_child_subreaper("));
}
