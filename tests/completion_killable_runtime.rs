//! Run the production completion and signal decision, replacing only the host's
//! scheduler and interrupt transport. EINTR decisions stay in production code.
#[test]
fn production_completion_observes_pending_sigkill() {
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
    let syscall = fs::read_to_string(root.join("kernel/src/syscall/mod.rs")).unwrap();
    let fixture = include_str!("fixtures/completion_killable.rs")
        .replace("@ROOT@", root.to_str().unwrap())
        .replace("@CHECK_SIGNALS@", &item(&syscall, "pub fn check_signals_for_eintr()"))
        .replace("@FATAL_SIGNAL@", &item(&syscall, "pub fn fatal_signal_pending()"))
        .replace("@WAIT_SIGNALS@", &item(&syscall, "fn signal_wait_signals()"));
    let dir = std::env::temp_dir().join(format!("completion-killable-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("check.rs");
    let binary = dir.join("check");
    fs::write(&source, fixture).unwrap();
    let mut build = Command::new("rustc");
    build.args(["--edition=2021", "--test", "-Dwarnings"]);
    // Run the x86 production wait branch on macOS ARM via Rosetta; its IRQ
    // transport is replaced above, so no privileged instruction executes.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    build.args(["--target", "x86_64-apple-darwin"]);
    let built = build.arg(&source).arg("-o").arg(&binary).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    // Each case owns the production module's static completion independently.
    for case in ["row_pending_sigkill_interrupts_before_sleep",
                 "uncollected_sigkill_interrupts_before_sleep",
                 "sigkill_between_preemption_release_and_block_interrupts",
                 "sigkill_after_block_interrupts_and_retires_waiter",
                 "sigterm_does_not_interrupt_a_killable_completion"] {
        let run = Command::new(&binary).args(["--exact", case]).output().unwrap();
        assert!(run.status.success(), "{case}: {}{}",
            String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    }
    fs::remove_dir_all(dir).unwrap();
}
