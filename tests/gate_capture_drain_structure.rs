//! Structural ratchet + functional oracle for failure-trace-capture PR-5:
//! `docker/qemu/lib/gate-capture-drain.sh` and its wiring into the gates
//! that source it. See
//! docs/planning/green-program/failure-capture/PLAN-2026-09-05.md section 6
//! (PR-5) and this round's own doc,
//! docs/planning/green-program/failure-capture/PR-5-2026-09-06.md.
//!
//! Three things are checked here, deliberately kept apart:
//!
//! 1. Census-anchored wiring: each gate script that sources the drain lib
//!    (not a literal list of four names -- the file set as it exists on
//!    disk) calls both `gcd_drain_and_report` and `gcd_pass_report`
//!    somewhere in its body.
//! 2. The actual ratchet: each "guest kill" line in a converted gate --
//!    `kill $QEMU_PID`/`kill "$QEMU_PID"`/`kill $RUNNER_PID`/`kill
//!    "$RUNNER_PID"`, each followed by `2>/dev/null`, the one shape each
//!    outcome-kill in these four scripts uses -- has a drain-decision call
//!    within the KILL_WINDOW lines immediately before it. A mutation test
//!    proves this actually reddens: relocating a real kill line to sit
//!    ABOVE its drain-decision block (literally "move the kill above the
//!    drain") makes the check fail where the unmutated file passes.
//! 3. A functional oracle over the shell library itself, run with no QEMU
//!    boot: the SAME underlying race -- a capture whose `END` line lands
//!    150ms after this check runs -- reads `partial` with
//!    `BREENIX_GATE_DRAIN_DISABLE=1` (the drain skips both waits, so it reads the
//!    file exactly as it stood) and `complete` with the drain enabled at its
//!    ordinary (tightened, for test speed) bounds (the drain waits long
//!    enough to see the `END` land). Both legs read the identical
//!    underlying serial content; only whether draining happened differs.
//!
//! # Why line-window text matching, not an AST
//!
//! These are bash scripts. A window-based textual check cannot see through
//! an indirection (a kill issued from a variable holding a function name,
//! say), the same limitation `capture_path_lock_free_structure.rs`'s own
//! denylist documents for its source scan. What it catches -- the actual
//! shape each kill site in these four scripts uses today, verified by the
//! census below finding six of them -- is what this PR shipped, and the
//! mutation test proves the check is not vacuous against that shape.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const GATE_DIR: &str = "docker/qemu";
const LIB_MARKER: &str = "lib/gate-capture-drain.sh";
const DRAIN_CALL: &str = "gcd_drain_and_report";
const PASS_CALL: &str = "gcd_pass_report";
/// Lines to look back from a kill site for a drain-decision call. Measured
/// against the six real call sites this PR wired: the widest gap (the
/// aarch64 gates' own "immediately before kill" host-facts sampling block,
/// which sits between the drain decision and the kill itself) is under 15
/// lines including comments; 30 leaves generous room without being so wide
/// it would still "cover" a kill line from an unrelated, much earlier
/// call.
const KILL_WINDOW: usize = 30;

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read(rel: &str) -> String {
    let full = repo_path(rel);
    fs::read_to_string(&full)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", full.display()))
}

/// The `.sh` files under `docker/qemu/`, recursively -- the same walk
/// `capture_path_lock_free_structure.rs::gate_scripts()` uses, duplicated
/// (not shared) because these are separate `rustc --test` compilation units
/// with no shared crate between them (`scripts/run-structure-tests.sh`
/// compiles each file standalone).
fn gate_scripts() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("docker/qemu must exist") {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("sh") {
                out.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    walk(&repo_path(GATE_DIR), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(repo_path(""))
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let body = fs::read_to_string(&path).expect("readable script");
            (name, body)
        })
        .collect()
}

/// Census-anchored, not a literal list: the gate scripts that source
/// `lib/gate-capture-drain.sh`, whatever that set currently is.
fn converted_gates() -> Vec<(String, String)> {
    gate_scripts()
        .into_iter()
        .filter(|(_, body)| body.contains(LIB_MARKER))
        .collect()
}

/// A "guest kill" line: `kill` (unquoted or quoted `$QEMU_PID`/`$RUNNER_PID`)
/// immediately followed by `2>/dev/null` on the same line. This is the exact
/// shape each outcome-kill in the four converted gates uses; it does NOT
/// match `kill -0 ...` liveness probes (different shape: no `2>/dev/null`
/// suffix on the same pattern) or a bare `wait`.
fn is_kill_line(line: &str) -> bool {
    let t = line.trim_start();
    let starts = t.starts_with("kill $QEMU_PID")
        || t.starts_with("kill \"$QEMU_PID\"")
        || t.starts_with("kill $RUNNER_PID")
        || t.starts_with("kill \"$RUNNER_PID\"");
    starts && t.contains("2>/dev/null")
}

fn count_kill_sites(lines: &[&str]) -> usize {
    lines.iter().filter(|l| is_kill_line(l)).count()
}

/// 0-based indices of kill-site lines with neither `gcd_drain_and_report`
/// nor `gcd_pass_report` anywhere in the KILL_WINDOW lines immediately
/// before them. Empty means each kill site in `lines` is covered.
fn uncovered_kill_sites(lines: &[&str]) -> Vec<usize> {
    let mut bad = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !is_kill_line(line) {
            continue;
        }
        let start = i.saturating_sub(KILL_WINDOW);
        let window = &lines[start..i];
        // Comment lines (trimmed, starting with `#`) do not count as
        // coverage -- this suite's own prose above the real call sites
        // mentions both function names, and a check that credited a
        // comment for coverage could not be reddened by relocating the
        // call it is explaining (caught live: the mutation test below
        // failed to redden until this line was added).
        let covered = window.iter().any(|l| {
            let t = l.trim_start();
            !t.starts_with('#') && (l.contains(DRAIN_CALL) || l.contains(PASS_CALL))
        });
        if !covered {
            bad.push(i);
        }
    }
    bad
}

#[test]
fn converted_gates_exist_and_call_both_drain_functions() {
    let gates = converted_gates();
    assert!(
        gates.len() >= 4,
        "expected at least 4 gate scripts under {GATE_DIR} to source {LIB_MARKER}, found {} \
         -- a census that collapsed to zero is not checking anything",
        gates.len()
    );
    for (name, body) in &gates {
        assert!(
            body.contains(DRAIN_CALL),
            "{name} sources the capture-drain lib but never calls {DRAIN_CALL}"
        );
        assert!(
            body.contains(PASS_CALL),
            "{name} sources the capture-drain lib but never calls {PASS_CALL}"
        );
    }
}

#[test]
fn every_guest_kill_site_is_preceded_by_a_drain_decision() {
    let gates = converted_gates();
    assert!(
        !gates.is_empty(),
        "no gate sources {LIB_MARKER} -- nothing for this test to check"
    );
    let mut total_kill_sites = 0usize;
    for (name, body) in &gates {
        let lines: Vec<&str> = body.lines().collect();
        total_kill_sites += count_kill_sites(&lines);
        let bad = uncovered_kill_sites(&lines);
        assert!(
            bad.is_empty(),
            "{name}: kill site(s) at 1-based line(s) {:?} have no {DRAIN_CALL}/{PASS_CALL} \
             call in the preceding {KILL_WINDOW} lines -- this gate can now send \
             SIGTERM/KILL before draining an open BXCAP capture",
            bad.iter().map(|i| i + 1).collect::<Vec<_>>()
        );
    }
    // Anti-vacuity floor: this PR wired exactly six guest-kill sites across
    // the four converted gates (see this suite's own file header). A count
    // below that floor would mean is_kill_line's shape stopped matching a
    // real site -- a missing site would trivially have "no uncovered site"
    // reported for it, because there is no site there to check.
    assert!(
        total_kill_sites >= 6,
        "expected at least 6 guest-kill sites across the converted gates, found \
         {total_kill_sites} -- a census that found none would make the check above vacuous"
    );
}

#[test]
fn moving_a_kill_above_its_drain_decision_reddens_the_check() {
    let body = read("docker/qemu/run-aarch64-boot-test-strict.sh");
    let mut lines: Vec<String> = body.lines().map(str::to_string).collect();

    let kill_idx = {
        let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
        borrowed
            .iter()
            .position(|l| is_kill_line(l))
            .expect("run-aarch64-boot-test-strict.sh must have a guest-kill line for this test")
    };
    let block_idx = lines
        .iter()
        .position(|l| l.contains("local CAPTURE_LINES"))
        .expect("run-aarch64-boot-test-strict.sh must declare CAPTURE_LINES before its kill line");
    assert!(
        block_idx < kill_idx,
        "the drain decision must originally precede the kill for 'moving the kill above it' \
         to mean anything (block at line {}, kill at line {})",
        block_idx + 1,
        kill_idx + 1
    );

    // Anti-vacuity: the REAL, unmutated file must be green first. If it
    // were already broken, the mutation below "reddening" it would prove
    // no more than that a broken check stayed broken.
    {
        let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
        let bad = uncovered_kill_sites(&borrowed);
        assert!(
            bad.is_empty(),
            "the real file must pass the kill-site check before this test's mutation is \
             applied; it did not (uncovered 0-based line indices: {bad:?})"
        );
    }

    // THE MUTATION: "move the kill above the drain", applied literally --
    // relocate the kill line to sit immediately before the drain-decision
    // block it used to follow; the rest of the file is unchanged.
    let kill_line = lines.remove(kill_idx);
    lines.insert(block_idx, kill_line);

    let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
    let bad = uncovered_kill_sites(&borrowed);
    assert!(
        !bad.is_empty(),
        "moving the kill line above its drain-decision block must make the kill-site check \
         red; it stayed green, which means the check is not actually anchored to this shape"
    );
}

/// Runs a small bash script with `docker/qemu/lib/gate-capture-drain.sh`
/// sourced ahead of it, returning (exit success, stdout).
fn run_with_drain_lib(script: &str) -> (bool, String) {
    let lib = repo_path("docker/qemu/lib/gate-capture-drain.sh");
    let full = format!("set -e\nsource '{}'\n{}\n", lib.display(), script);
    let output = Command::new("bash")
        .arg("-c")
        .arg(&full)
        .output()
        .expect("failed to spawn bash");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
    )
}

fn unique_temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gate-capture-drain-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn classify_pairs_a_begin_and_end_split_across_two_serial_files_by_seq() {
    let dir = unique_temp_dir("multifile");

    // x86's own shape: kernel-console BXCAP output on one file, the BEGIN
    // and one EV record; the END lands on the OTHER file, the way it would
    // if a peer CPU's write interleaved onto the second `-serial file:`
    // sink. This fixture carries no EV records in either file, so it also
    // covers gcd_last_events's empty-ring reading for a capture whose ring
    // copied out clean.
    let kernel = dir.join("serial_kernel.txt");
    fs::write(
        &kernel,
        "[BXCAP:BEGIN v=1 seq=9 edge=FAULT cpu=0 ts=1 tsfreq=1 uptime_ms=1 arch=x86_64]
",
    )
    .unwrap();
    let user = dir.join("serial_user.txt");
    fs::write(
        &user,
        "[BXCAP:END v=1 seq=9 edge=FAULT verdict=complete records=1 bytes=90 truncated=0 sections_skipped=0x0]
",
    )
    .unwrap();

    let (ok, out) = run_with_drain_lib(&format!(
        "gcd_classify '{}' '{}'",
        kernel.display(),
        user.display()
    ));
    assert!(ok, "gcd_classify failed on the split-file fixture");
    assert_eq!(
        out.trim(),
        "complete 9 FAULT 0 1",
        "a BEGIN on one file and its matching END (by seq=) on the other must classify as complete"
    );

    let (ok, out) = run_with_drain_lib(&format!(
        "gcd_last_events 8 '{}' '{}'",
        kernel.display(),
        user.display()
    ));
    assert!(ok, "gcd_last_events failed on the split-file fixture");
    assert_eq!(
        out.trim(),
        "none",
        "a capture with zero [BXCAP:EV] records across either file must read none, not an empty string"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn classify_distinguishes_complete_partial_and_absent_with_no_boot() {
    let dir = unique_temp_dir("classify");

    let complete = dir.join("complete.txt");
    fs::write(
        &complete,
        "[BXCAP:BEGIN v=1 seq=0 edge=SELFTEST cpu=0 ts=1 tsfreq=1 uptime_ms=1 arch=aarch64]\n\
         [BXCAP:EV cpu=0 i=0 ts=1 type=0x1 n=TIMER_TICK p=16 f=0x0]\n\
         [BXCAP:END v=1 seq=0 edge=SELFTEST verdict=complete records=5 bytes=400 truncated=0 sections_skipped=0x0]\n",
    )
    .unwrap();

    let partial = dir.join("partial.txt");
    fs::write(
        &partial,
        "[BXCAP:BEGIN v=1 seq=7 edge=FAULT cpu=1 ts=1 tsfreq=1 uptime_ms=1 arch=aarch64]\n\
         [BXCAP:EV cpu=1 i=0 ts=1 type=0x2 n=CTX_SWITCH p=42 f=0x0]\n",
    )
    .unwrap();

    let absent = dir.join("absent.txt");
    fs::write(&absent, "plain boot output, no capture at all\n").unwrap();

    let (ok, out) = run_with_drain_lib(&format!("gcd_classify '{}'", complete.display()));
    assert!(ok, "gcd_classify failed on the complete fixture");
    assert_eq!(out.trim(), "complete 0 SELFTEST 0 5");

    let (ok, out) = run_with_drain_lib(&format!("gcd_classify '{}'", partial.display()));
    assert!(ok, "gcd_classify failed on the partial fixture");
    assert_eq!(out.trim(), "partial 7 FAULT 1 -");

    let (ok, out) = run_with_drain_lib(&format!("gcd_classify '{}'", absent.display()));
    assert!(ok, "gcd_classify failed on the absent fixture");
    assert_eq!(out.trim(), "absent - - - -");

    fs::remove_dir_all(&dir).ok();
}

/// The oracle: the SAME race -- a capture whose `END` line lands 150ms after
/// this check runs -- read two ways. With draining disabled it reads
/// `partial` (the mutation this PR's plan describes as "move the kill above
/// the drain"); with draining enabled, at bounds tightened for test speed
/// but otherwise the real code path, it reads `complete`. No QEMU boot: the
/// race is a background `sh` process appending real BXCAP-shaped lines to a
/// file this test also reads.
#[test]
fn drain_disabled_reads_partial_drain_enabled_reads_complete_same_race() {
    let dir = unique_temp_dir("oracle");

    let begin = "[BXCAP:BEGIN v=1 seq=3 edge=FAULT cpu=0 ts=1 tsfreq=1 uptime_ms=1 arch=aarch64]\n\
                 [BXCAP:EV cpu=0 i=0 ts=1 type=0x2 n=CTX_SWITCH p=7 f=0x0]\n";
    let rest = "[BXCAP:EV cpu=0 i=1 ts=2 type=0x2 n=CTX_SWITCH p=8 f=0x0]\n\
                [BXCAP:END v=1 seq=3 edge=FAULT verdict=complete records=3 bytes=200 truncated=0 sections_skipped=0x0]\n";

    // Leg A: BREENIX_GATE_DRAIN_DISABLE=1. Reads the file the instant the
    // capture-drain call runs, before the appender's 150ms-delayed write.
    let leg_a = dir.join("leg_a.txt");
    fs::write(&leg_a, begin).unwrap();
    let script_a = format!(
        "( sleep 0.15; printf '%s' '{rest}' >> '{path}' ) &\n\
         BREENIX_GATE_DRAIN_DISABLE=1 gcd_drain_and_report '{path}'\n\
         wait",
        rest = rest.replace('\'', "'\\''"),
        path = leg_a.display()
    );
    let (ok, out_a) = run_with_drain_lib(&script_a);
    assert!(ok, "leg A (drain disabled) failed to run: {out_a}");
    assert!(
        out_a.contains("[CAPTURE_DRAIN:capture=partial:"),
        "leg A (BREENIX_GATE_DRAIN_DISABLE=1) must read capture=partial -- it read:\n{out_a}"
    );

    // Leg B: same race, drain enabled, bounds tightened only for test
    // speed (settle+quiet+max still comfortably outlast the 150ms delay).
    let leg_b = dir.join("leg_b.txt");
    fs::write(&leg_b, begin).unwrap();
    let script_b = format!(
        "( sleep 0.15; printf '%s' '{rest}' >> '{path}' ) &\n\
         BREENIX_GATE_DRAIN_SETTLE_MS=50 BREENIX_GATE_DRAIN_QUIET_MS=100 \
         BREENIX_GATE_DRAIN_MAX_MS=2000 gcd_drain_and_report '{path}'\n\
         wait",
        rest = rest.replace('\'', "'\\''"),
        path = leg_b.display()
    );
    let (ok, out_b) = run_with_drain_lib(&script_b);
    assert!(ok, "leg B (drain enabled) failed to run: {out_b}");
    assert!(
        out_b.contains("[CAPTURE_DRAIN:capture=complete:"),
        "leg B (drain enabled, same race) must read capture=complete -- it read:\n{out_b}"
    );

    fs::remove_dir_all(&dir).ok();
}
