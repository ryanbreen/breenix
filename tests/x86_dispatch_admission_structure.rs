use std::{fs, path::PathBuf};

fn source(path: &str) -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

fn body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source.find(signature).expect(signature);
    let open = start + source[start..].find('{').unwrap();
    let mut depth = 1;
    for (offset, ch) in source[open + 1..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return &source[open + 1..open + 1 + offset];
        }
    }
    panic!("unterminated {signature}")
}

fn admission(scheduler: &str, dispatch: &str) -> bool {
    let Some(start) = scheduler.find("#[cfg(not(target_arch = \"aarch64\"))]\npub fn schedule()")
    else {
        return false;
    };
    if body(&scheduler[start..], "pub fn schedule()").trim() != "set_need_resched();" {
        return false;
    }
    scheduler.contains("pub(crate) fn schedule_for_interrupt_return(\n    _: &crate::interrupts::context_switch::InterruptDispatch,")
        && dispatch.contains("pub(crate) struct InterruptDispatch {\n    private: (),\n}")
        && dispatch.contains("scheduler::schedule_for_interrupt_return(&admission)")
        && !dispatch.contains("impl Default for InterruptDispatch")
        && !scheduler.contains("pub fn schedule(&mut self)")
}

#[test]
fn v2_selection_requires_dispatch_admission_and_public_schedule_only_requests() {
    let scheduler = source("kernel/src/task/scheduler.rs");
    let dispatch = source("kernel/src/interrupts/context_switch.rs");
    assert!(admission(&scheduler, &dispatch));
    for (s, d) in [
        (
            scheduler.replace(
                "pub fn schedule() {\n    set_need_resched();",
                "pub fn schedule() {\n    let _ = with_scheduler(|s| s.schedule());",
            ),
            dispatch.clone(),
        ),
        (
            scheduler.replace(
                "_: &crate::interrupts::context_switch::InterruptDispatch,",
                "",
            ),
            dispatch.clone(),
        ),
        (
            scheduler.clone(),
            dispatch.replace("    private: (),", "    pub(crate) private: (),"),
        ),
        (
            scheduler.clone(),
            dispatch.replace(
                "scheduler::schedule_for_interrupt_return(&admission)",
                "scheduler::schedule()",
            ),
        ),
    ] {
        assert!(s != scheduler || d != dispatch);
        assert!(!admission(&s, &d), "admission mutation accepted");
    }
}

fn silent(source: &str) -> bool {
    let dispatch = source.split("pub fn idle_loop()").next().unwrap();
    ![
        "raw_serial",
        "log::",
        "serial_print",
        "Port::new",
        "0x3F8",
        "out dx",
        "println!",
        "print!",
        "format!",
    ]
    .iter()
    .any(|needle| dispatch.contains(needle))
}

#[test]
fn v3_dispatch_has_no_serial_or_formatting_path() {
    let dispatch = source("kernel/src/interrupts/context_switch.rs");
    assert!(silent(&dispatch));
    for call in [
        "raw_serial_str(\"[SW]\");",
        "log::trace!(\"switch\");",
        "Port::new(0x3F8);",
        "core::arch::asm!(\"out dx, al\");",
        "serial_println!(\"switch\");",
    ] {
        let mutated = dispatch.replacen(
            "fn switch_to_thread(",
            &format!("{call}\nfn switch_to_thread("),
            1,
        );
        assert_ne!(dispatch, mutated);
        assert!(!silent(&mutated));
    }
}

#[test]
fn v2_real_public_entry_runs_and_dispatch_permit_is_private() {
    use std::process::Command;
    let scheduler = source("kernel/src/task/scheduler.rs");
    let dispatch = source("kernel/src/interrupts/context_switch.rs");
    let start = scheduler
        .find("#[cfg(not(target_arch = \"aarch64\"))]\npub fn schedule()")
        .unwrap();
    let real_body = body(&scheduler[start..], "pub fn schedule()");
    let permit_body = body(&dispatch, "pub(crate) struct InterruptDispatch");
    let dir = std::env::temp_dir().join(format!("dispatch-admission-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("probe.rs");
    let output = dir.join("probe");
    let compile = |text: &str| {
        fs::write(&input, text).unwrap();
        Command::new("rustc")
            .args([
                "--edition=2021",
                "--crate-name",
                "dispatch_probe",
                "-Dwarnings",
            ])
            .arg(&input)
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap()
    };
    let good = format!(
        r#"
use std::sync::atomic::{{AtomicBool, Ordering}};
static NEED_RESCHED: AtomicBool = AtomicBool::new(false);
fn set_need_resched() {{ NEED_RESCHED.store(true, Ordering::Relaxed); }}
fn schedule() {{ {real_body} }}
mod dispatch {{
    pub(crate) struct InterruptDispatch {{ {permit_body} }}
    pub fn admitted() {{
        let admission = InterruptDispatch {{ private: () }};
        let () = admission.private;
        super::select(&admission);
    }}
}}
fn select(_: &dispatch::InterruptDispatch) {{}}
fn main() {{ schedule(); assert!(NEED_RESCHED.load(Ordering::Relaxed)); dispatch::admitted(); }}
"#
    );
    let result = compile(&good);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(Command::new(&output).status().unwrap().success());
    let bad = good.replace(
        "dispatch::admitted();",
        "select(&dispatch::InterruptDispatch { private: () }); dispatch::admitted();",
    );
    let result = compile(&bad);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("E0451"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn boot_markers_require_observed_dispatch_and_enabled_thread_context() {
    let facts = source("kernel/src/task/dispatch_boot_facts.rs");
    let dispatch = source("kernel/src/interrupts/context_switch.rs");
    let reporter = source("kernel/src/task/dispatch_strand_census.rs");
    let note = body(&facts, "pub(crate) fn note(");
    assert!(silent(note));
    assert!(note.contains("SEEN.fetch_or(bit, Ordering::Release)"));
    assert!(dispatch.contains("note_boot_fact(BootDispatchFact::ScheduleReturned)"));
    assert!(dispatch.contains(
        "if from_userspace {\n            note_boot_fact(BootDispatchFact::UserspaceReturned)"
    ));
    let emitter = body(&facts, "pub(crate) fn emit_if_enabled()");
    let guard = emitter
        .find("if !crate::arch_interrupts_enabled() {\n        return;")
        .unwrap();
    assert!(guard < emitter.find("SEEN.load(Ordering::Acquire)").unwrap());
    assert!(emitter.contains("is_ring3_confirmed()"));
    assert!(emitter.contains("EMITTED.fetch_or(seen, Ordering::Relaxed)"));
    assert!(reporter.contains("super::dispatch_boot_facts::emit_if_enabled();"));
}

#[test]
fn deferred_marker_code_waits_for_if_and_syscall_then_emits_once() {
    use std::process::Command;
    let module =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("kernel/src/task/dispatch_boot_facts.rs");
    let directory = std::env::temp_dir().join(format!("boot-facts-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let input = directory.join("probe.rs");
    let output = directory.join("probe");
    fs::write(&input, format!(r#"
use std::sync::{{Mutex, atomic::{{AtomicBool, Ordering}}}};
static IF: AtomicBool = AtomicBool::new(false);
static SYSCALL: AtomicBool = AtomicBool::new(false);
static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
fn arch_interrupts_enabled() -> bool {{ IF.load(Ordering::Relaxed) }}
mod syscall {{ pub mod handler {{ pub fn is_ring3_confirmed() -> bool {{ super::super::SYSCALL.load(super::super::Ordering::Relaxed) }} }} }}
#[macro_export]
macro_rules! serial_println {{ ($($arg:tt)*) => {{ crate::LINES.lock().unwrap().push(format!($($arg)*)); }}; }}
#[path = {module:?}] mod facts;
fn main() {{
    use facts::BootDispatchFact::*;
    facts::note(ScheduleReturned); facts::note(UserspaceReturned);
    facts::emit_if_enabled(); assert!(LINES.lock().unwrap().is_empty());
    IF.store(true, Ordering::Relaxed);
    facts::emit_if_enabled(); assert_eq!(LINES.lock().unwrap().len(), 1);
    facts::emit_if_enabled(); assert_eq!(LINES.lock().unwrap().len(), 1);
    SYSCALL.store(true, Ordering::Relaxed);
    facts::emit_if_enabled(); assert_eq!(LINES.lock().unwrap().len(), 3);
    facts::note(UserspaceReturned); facts::emit_if_enabled();
    assert_eq!(LINES.lock().unwrap().len(), 3);
}}
"#)).unwrap();
    let result = Command::new("rustc")
        .args([
            "--edition=2021",
            "--crate-name",
            "boot_facts_probe",
            "-Dwarnings",
        ])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(Command::new(&output).status().unwrap().success());
    fs::remove_dir_all(directory).unwrap();
}
