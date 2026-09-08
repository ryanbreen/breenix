//! Issue 927: the supplied-page-table fork must reach CoW and completion in
//! boot_tests without selecting the testing loader. These are source checks;
//! the companion gate exercises the unchanged retirement counters at runtime.
use std::{fs, path::PathBuf};

fn root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")) }
fn fork_body(source: &str) -> &str {
    let start = source.find("pub fn fork_process_with_page_table(").expect("fork helper");
    let end = source[start..].find("    /// Fork a process with the ACTUAL").expect("next helper");
    &source[start..start + end]
}
fn validate_fork(source: &str) -> Result<(), &'static str> {
    let body = fork_body(source);
    if body.contains("#[cfg") || body.contains("cfg!(") {
        return Err("supplied-page-table fork must not depend on a feature");
    }
    if body.contains("Cannot implement fork without testing feature") {
        return Err("non-testing fork refusal restored");
    }
    for required in ["super::fork::setup_cow_pages_with_vmas(",
                     "child_process.page_table = Some(child_page_table);", "self.complete_fork("] {
        if !body.contains(required) { return Err("fork work missing"); }
    }
    let restore = body.find("parent.page_table = Some(parent_page_table);").ok_or("parent restore missing")?;
    let propagate = body.find("let pages_shared = cow_result?;").ok_or("CoW result missing")?;
    if restore > propagate { return Err("CoW error would remove the parent's page table"); }
    Ok(())
}
#[test]
fn supplied_page_table_fork_is_profile_independent() {
    let source = fs::read_to_string(root().join("kernel/src/process/manager.rs")).unwrap();
    assert_eq!(validate_fork(&source), Ok(()));
}
#[test]
fn mutation_testing_only_cow_is_rejected() {
    let source = fs::read_to_string(root().join("kernel/src/process/manager.rs")).unwrap();
    let mutated = source.replacen("// COPY-ON-WRITE FORK: Share pages between parent and child\n",
        "// COPY-ON-WRITE FORK: Share pages between parent and child\n        #[cfg(feature = \"testing\")]\n", 1);
    assert_ne!(source, mutated);
    assert!(validate_fork(&mutated).is_err());
}
#[test]
fn mutation_missing_completion_is_rejected() {
    let source = fs::read_to_string(root().join("kernel/src/process/manager.rs")).unwrap();
    let mutated = source.replacen("self.complete_fork(", "removed_completion(", 1);
    assert_ne!(source, mutated);
    assert!(validate_fork(&mutated).is_err());
}

#[test]
fn gate_builds_only_boot_tests_and_reuses_cohort_pins() {
    let gate = fs::read_to_string(root().join("docker/qemu/run-x86-boot-tests-only.sh")).unwrap();
    assert!(gate.contains("cargo build --release --features boot_tests --bin qemu-uefi"));
    assert!(!gate.contains("--features boot_tests,"));
    assert!(!gate.contains("BREENIX_GATE_SKIP_STRUCTURE"));
    assert!(gate.contains("if ! gate_structure_preflight \"$BREENIX_ROOT\" \"$BREENIX_GATE_TMP\""));
    for pin in ["PT_COHORT_LITERAL", "PT_EXEC_COHORT_LITERAL", "TOMBSTONE_JOIN_ORACLE_LITERAL"] {
        assert!(gate.contains(pin));
    }
    assert!(gate.contains("serial.count(pins[0]) == 1"));
    assert!(gate.contains("serial.count(marker) == 1"));
    assert!(gate.contains("elapsed<900"));
    assert!(gate.contains("qemu_host_lock_track_pid \"$QEMU_PID\""));
}

#[test]
fn mutation_cow_error_before_parent_restore_is_rejected() {
    let source = fs::read_to_string(root().join("kernel/src/process/manager.rs")).unwrap();
    let mutated = source.replacen("parent.page_table = Some(parent_page_table);\n            let pages_shared = cow_result?;",
        "let pages_shared = cow_result?;\n            parent.page_table = Some(parent_page_table);", 1);
    assert_ne!(source, mutated);
    assert!(validate_fork(&mutated).is_err());
}

#[test]
fn x86_oracle_allows_the_same_cohort_startup_window() {
    let gate = fs::read_to_string(root().join("docker/qemu/run-blocking-io-oracle-gate.sh")).unwrap();
    assert!(gate.contains("HOST_DEADLINE=120"));
    assert!(gate.contains("if [ \"$ARCH\" = x86_64 ]; then HOST_DEADLINE=900; fi"));
    assert_eq!(gate.matches("[ \"$elapsed\" -lt \"$HOST_DEADLINE\" ]").count(), 2);
}

fn validate_context_fork(source: &str) -> Result<(), &'static str> {
    let start = source.find("pub fn fork_process_with_context(").ok_or("context fork missing")?;
    let end = source[start..].find("    /// Replace a process's address space").ok_or("next method missing")?;
    let body = &source[start..start+end];
    if body.contains("#[cfg") || body.contains("cfg!(") { return Err("context fork is feature gated"); }
    for required in ["super::fork::setup_cow_pages_with_vmas(",
        "child_process.page_table = Some(child_page_table);",
        "child_process.set_main_thread(child_thread);", "self.processes.insert(child_pid, child_process);"] {
        if !body.contains(required) { return Err("context fork work missing"); }
    }
    let restore = body.find("parent_mut.page_table = Some(parent_page_table);").ok_or("parent restore missing")?;
    let propagate = body.find("let pages_shared = cow_result?;").ok_or("CoW result missing")?;
    if restore > propagate { return Err("CoW error precedes parent restoration"); }
    Ok(())
}
#[test]
fn context_fork_is_profile_independent() {
    let source = fs::read_to_string(root().join("kernel/src/process/manager.rs")).unwrap();
    assert_eq!(validate_context_fork(&source), Ok(()));
}
#[test]
fn mutation_testing_only_context_fork_is_rejected() {
    let source = fs::read_to_string(root().join("kernel/src/process/manager.rs")).unwrap();
    let start = source.find("pub fn fork_process_with_context(").unwrap();
    let mut mutated = source[..start].to_string();
    mutated.push_str(&source[start..].replacen("        // COPY-ON-WRITE FORK: Share pages between parent and child\n",
        "        // COPY-ON-WRITE FORK: Share pages between parent and child\n        #[cfg(feature = \"testing\")]\n", 1));
    assert_ne!(source, mutated);
    assert!(validate_context_fork(&mutated).is_err());
}
#[test]
fn mutation_context_cow_error_before_restore_is_rejected() {
    let source = fs::read_to_string(root().join("kernel/src/process/manager.rs")).unwrap();
    let mutated = source.replacen("parent_mut.page_table = Some(parent_page_table);\n            let pages_shared = cow_result?;",
        "let pages_shared = cow_result?;\n            parent_mut.page_table = Some(parent_page_table);", 1);
    assert_ne!(source, mutated);
    assert!(validate_context_fork(&mutated).is_err());
}

#[test]
fn oracle_scorer_distinguishes_gdt_setup_from_faults_without_weakening_arms() {
    use std::process::Command;
    let gate = fs::read_to_string(root().join("docker/qemu/run-blocking-io-oracle-gate.sh")).unwrap();
    assert!(gate.contains("python3 \"$BREENIX_ROOT/scripts/score-blocking-io-oracle.py\" \"$ARCH\" \"$RUN_DIR\" \"${EXPECTED_ARMS[@]}\""));
    let scorer = fs::read_to_string(root().join("scripts/score-blocking-io-oracle.py")).unwrap();
    let dir = std::env::temp_dir().join(format!("breenix-927-scorer-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let good = "[DEBUG] kernel::gdt:   TSS IST[0] (double fault stack): 0x0\n\
        [ INFO] kernel::gdt: Updated IST[0] (double fault stack) to 0xffffc98000002000\n\
        [PIPE_WRITE_ORACLE:x86_64:pipe:full_block:verdict=PASS:bytes=4096:expected=4096]\n\
        [PIPE_WRITE_ORACLE:x86_64:fifo:full_block:verdict=PASS:bytes=4096:expected=4096]\n\
        [PIPE_WRITE_SUMMARY:x86_64:passed=2:failed=0]\n\
        [PIPE_WRITE_RESULT:x86_64:status=0]\n";
    let mut good = good.to_string();
    let console_arms = [("blocking", 1), ("nonblock_open", 0), ("nonblock_fcntl", 0),
        ("readiness_partial", 2), ("eintr", 1), ("immediate", 8)];
    for device in ["/dev/console", "/dev/tty"] {
        for (arm, bytes) in console_arms {
            good.push_str(&format!("[CONSOLE_READ_ORACLE:x86_64:{device}:{arm}:verdict=PASS:bytes={bytes}]\n"));
        }
    }
    good.push_str("[CONSOLE_READ_SUMMARY:x86_64:passed=12:failed=0]\n");
    let good = good.as_str();
    let run = |source: &str, serial: &str| {
        fs::write(dir.join("serial.txt"), serial).unwrap();
        Command::new("python3").args(["-c", source, "x86_64"])
            .arg(&dir).arg("full_block").arg("--console")
            .args(console_arms.iter().map(|(arm, _)| *arm)).output().unwrap()
    };
    let result = run(&scorer, good);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    // Revert the crash-text fix: routine GDT init must redden this regression.
    let reverted = scorer.replace("crash_text, re.I", "text, re.I");
    assert_ne!(reverted, scorer);
    assert!(!run(&reverted, good).status.success());
    for fault in ["==================== DOUBLE FAULT ====================", "EXCEPTION: DOUBLE FAULT",
        "KERNEL PANIC", "TRIPLE FAULT", "DATA_ABORT", "INSTRUCTION_ABORT", "soft lockup detected",
        "TSS IST[0] (double fault stack): 0x0 DOUBLE FAULT"] {
        let result = run(&scorer, &format!("{good}{fault}\n"));
        assert!(!result.status.success(), "accepted {fault}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("kernel crash"));
    }
    for bad in [good.replace("/dev/tty:blocking", "/dev/tty:missing_arm"),
        good.replace("blocking:verdict=PASS:bytes=1", "blocking:verdict=PASS:bytes=0"),
        good.replace("CONSOLE_READ_SUMMARY:x86_64:passed=12", "CONSOLE_READ_SUMMARY:x86_64:passed=11"),
        good.replace("fifo:full_block", "fifo:missing_arm"),
        good.replace("bytes=4096", "bytes=4095"), good.replace("status=0", "status=1"),
        good.replace("verdict=PASS", "verdict=FAIL"), good.replace("passed=2", "passed=1"),
        format!("{good}[PIPE_WRITE_RESULT:x86_64:status=0]\n")] {
        assert!(!run(&scorer, &bad).status.success(), "accepted invalid oracle evidence");
    }
    fs::remove_dir_all(dir).unwrap();
}
