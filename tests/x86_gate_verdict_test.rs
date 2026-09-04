use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct SerialFixture {
    directory: PathBuf,
    kernel_log: PathBuf,
    user_log: PathBuf,
}

impl SerialFixture {
    fn new(kernel: &str, user: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "breenix-x86-gate-verdict-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap_or_else(|error| {
            panic!("create fixture directory {}: {error}", directory.display())
        });

        let kernel_log = directory.join("serial_kernel.txt");
        let user_log = directory.join("serial_user.txt");
        fs::write(&kernel_log, kernel)
            .unwrap_or_else(|error| panic!("write fixture {}: {error}", kernel_log.display()));
        fs::write(&user_log, user)
            .unwrap_or_else(|error| panic!("write fixture {}: {error}", user_log.display()));

        Self {
            directory,
            kernel_log,
            user_log,
        }
    }

    fn run(&self, expected_exits: Option<&str>) -> Output {
        self.run_in_order(expected_exits, false)
    }

    /// `user_first` swaps the argument order the gate scripts use, which is the
    /// round-3 F9 case: the verdict must not depend on concatenation order.
    fn run_in_order(&self, expected_exits: Option<&str>, user_first: bool) -> Output {
        let (first, second) = if user_first {
            (&self.user_log, &self.kernel_log)
        } else {
            (&self.kernel_log, &self.user_log)
        };
        let mut command = Command::new("bash");
        command
            .arg(repo_root().join("scripts/x86-gate-verdict.sh"))
            .arg(first)
            .arg(second)
            .current_dir(repo_root())
            .env_remove("EXPECTED_EXITS");
        if let Some(expected_exits) = expected_exits {
            command.env("EXPECTED_EXITS", expected_exits);
        }
        command.output().expect("run scripts/x86-gate-verdict.sh")
    }
}

impl Drop for SerialFixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.directory) {
            eprintln!(
                "failed to clean fixture directory {}: {error}",
                self.directory.display()
            );
        }
    }
}

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A capture committed in this repository, addressed from the repo root.
fn committed(relative: &str) -> PathBuf {
    repo_root().join(relative)
}

/// Run the census tool directly, in the argument order given. Used by the
/// tests that read committed captures rather than synthesised fixtures.
fn run_census(paths: &[&Path]) -> Output {
    let mut command = Command::new("bash");
    command.arg(repo_root().join("scripts/x86-strand-census.sh"));
    for path in paths {
        command.arg(path);
    }
    command.current_dir(repo_root());
    command.output().expect("run scripts/x86-strand-census.sh")
}

/// One snapshot in the shape `report_snapshot()` emits.
fn marker(seq: u32, tick: u32, ms: u32, saved: u32, stranded: u32, tids: &str) -> String {
    format!(
        "[DISPATCH_STRAND_CENSUS:seq={seq}:tick={tick}:ms={ms}:saved={saved}:\
         stranded={stranded}:tids={tids}:tid_overflow=0:ledger_overflow=0]"
    )
}

/// The tail a capture that reaches the end of the userspace phase carries.
/// `completion` is the snapshot `sys_userspace_test_complete` emits on the line
/// after the marker: 4 of 4 committed captures that carry both a completion
/// marker and any census snapshot carry one there, so a fixture without it is
/// modelling a TRUNCATED capture, not a finished boot.
/// claim-lint:ok: the 4 are round3/r3-head-green, round3/r3-idle-cadence and
/// round4/gate-green/boot{1,2} under
/// docs/planning/green-program/sockets/serials/775/.
fn tail_pass_with(completion: &str) -> String {
    format!(
        "USERSPACE TEST COMPLETE\n{completion}\n\
         TEST_TALLY: exited=10 nonzero=0 failed=[]\n\
         \u{1f3c1} TEST RUNNER: All tests passed\n"
    )
}

/// The same tail with NO completion snapshot after the marker: the truncated
/// shape, which the census now rejects rather than reporting as "no marker".
fn tail_pass_truncated() -> &'static str {
    "USERSPACE TEST COMPLETE\n\
     TEST_TALLY: exited=10 nonzero=0 failed=[]\n\
     \u{1f3c1} TEST RUNNER: All tests passed\n"
}

fn green_log() -> String {
    format!(
        "{}\n{}",
        marker(1, 200, 1000, 11, 0, "-"),
        tail_pass_with(&marker(2, 400, 2000, 11, 0, "-"))
    )
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn accepts_a_complete_green_cohort_at_the_expected_floor() {
    let fixture = SerialFixture::new(&green_log(), "");
    let output = fixture.run(Some("10"));

    assert!(
        output.status.success(),
        "expected green verdict, got: {}",
        output_text(&output)
    );
    assert!(output_text(&output).contains("expected>=10"));
}

#[test]
fn rejects_a_tally_below_the_expected_exit_floor() {
    let fixture = SerialFixture::new(&green_log(), "");
    let output = fixture.run(Some("11"));
    let text = output_text(&output);

    assert!(!output.status.success(), "unexpected green verdict: {text}");
    assert!(
        text.contains("exited=10 below the expected floor EXPECTED_EXITS=11"),
        "floor failure did not name both counts: {text}"
    );
}

#[test]
fn rejects_a_missing_expected_exits_variable() {
    let fixture = SerialFixture::new(&green_log(), "");
    let output = fixture.run(None);
    let text = output_text(&output);

    assert!(!output.status.success(), "unexpected green verdict: {text}");
    assert!(
        text.contains("EXPECTED_EXITS must be set"),
        "missing-variable failure was unclear: {text}"
    );
}

#[test]
fn rejects_empty_non_decimal_and_zero_expected_exits_values() {
    let fixture = SerialFixture::new(&green_log(), "");

    for invalid in ["", "ten", "0"] {
        let output = fixture.run(Some(invalid));
        let text = output_text(&output);
        assert!(
            !output.status.success(),
            "EXPECTED_EXITS={invalid:?} unexpectedly passed: {text}"
        );
        assert!(
            text.contains("EXPECTED_EXITS must be set"),
            "EXPECTED_EXITS={invalid:?} produced an unclear failure: {text}"
        );
    }
}

#[test]
fn rejects_a_fault_killed_test_and_names_the_process() {
    let fixture = SerialFixture::new(
        &format!(
            "{}\nUSERSPACE TEST COMPLETE\n{}\n\
             TEST_TALLY: exited=10 nonzero=1 failed=[brk_test:-11]\n\
             TEST RUNNER: FAILED\n",
            marker(1, 200, 1000, 11, 0, "-"),
            marker(2, 400, 2000, 11, 0, "-")
        ),
        "",
    );
    let output = fixture.run(Some("10"));
    let text = output_text(&output);

    assert!(!output.status.success(), "unexpected green verdict: {text}");
    assert!(
        text.contains("brk_test"),
        "fault-killed process was not named by the failure: {text}"
    );
}

#[test]
fn rejects_a_strand_and_names_the_thread_from_the_scheduler_table() {
    let fixture = SerialFixture::new(
        &format!(
            "Added thread 23 'poll_tcp_oracle' to scheduler (user: true, target_cpu: 0)\n{}\n",
            marker(1, 200, 1000, 13, 1, "23")
        ),
        "",
    );
    let output = fixture.run(Some("10"));
    let text = output_text(&output);

    assert!(!output.status.success(), "unexpected green verdict: {text}");
    assert!(
        text.contains(
            "thread 23 (poll_tcp_oracle) saved blocked and not restored as of the latest snapshot"
        ),
        "strand failure did not name its thread: {text}"
    );
    assert!(
        text.contains("saved blocked in a kernel wait and was still not restored at the latest census snapshot"),
        "strand failure did not retain the gate reason: {text}"
    );
    assert!(
        !text.contains("never restored"),
        "the verdict overclaimed what one snapshot supports: {text}"
    );
}

#[test]
fn unavailable_census_falls_through_to_the_real_first_cause() {
    let fixture = SerialFixture::new("OVMF: early boot stopped\n", "");
    let output = fixture.run(Some("10"));
    let text = output_text(&output);

    assert!(!output.status.success(), "unexpected green verdict: {text}");
    assert!(
        text.contains("census unavailable"),
        "rc=2 was not identified: {text}"
    );
    assert!(
        text.contains("USERSPACE TEST COMPLETE was absent; boot did not finish"),
        "early failure did not reach the existing first-cause check: {text}"
    );
    assert!(
        !text.contains("saved blocked in a kernel wait"),
        "unavailable census was misclassified as a strand: {text}"
    );
}

#[test]
fn highest_seq_snapshot_wins_even_when_two_share_a_physical_line() {
    // The two markers that share a line are the pair AFTER the completion
    // marker, deliberately: with the clean one last on the line, a parser that
    // read only the first marker per line would judge the red seq=2 and go
    // red, so the assertion below still has teeth now that each green fixture
    // carries a completion snapshot.
    let kernel = format!(
        "{}\nUSERSPACE TEST COMPLETE\n{} {}\n\
         TEST_TALLY: exited=10 nonzero=0 failed=[]\n\
         \u{1f3c1} TEST RUNNER: All tests passed\n",
        marker(1, 200, 1000, 13, 0, "-"),
        marker(2, 400, 2000, 13, 1, "23"),
        marker(3, 600, 2100, 13, 0, "-")
    );
    let fixture = SerialFixture::new(&kernel, "");
    let output = fixture.run(Some("10"));

    assert!(
        output.status.success(),
        "newest snapshot did not supersede an earlier red one: {}",
        output_text(&output)
    );
}

#[test]
fn the_verdict_does_not_depend_on_argument_order() {
    // #775 round 3, F9: the red snapshot is in the KERNEL capture and a later
    // green-looking file is passed alongside it. Concatenation order used to
    // decide the reading; the highest seq decides it now.
    let kernel = format!(
        "Added thread 23 'poll_tcp_oracle' to scheduler (user: true, target_cpu: 0)\n{}\n{}",
        marker(2, 400, 2000, 13, 1, "23"),
        tail_pass_with(&marker(3, 600, 2500, 13, 1, "23"))
    );
    let user = format!("{}\n", marker(1, 200, 1000, 5, 0, "-"));
    let fixture = SerialFixture::new(&kernel, &user);

    for user_first in [false, true] {
        let output = fixture.run_in_order(Some("10"), user_first);
        let text = output_text(&output);
        assert!(
            !output.status.success(),
            "argument order {user_first} hid the strand: {text}"
        );
        assert!(
            text.contains("thread 23 (poll_tcp_oracle)"),
            "argument order {user_first} lost the thread name: {text}"
        );
    }
}

#[test]
fn a_malformed_trailing_marker_does_not_discard_the_red_reading() {
    // #775 round 3, N6: a tail mangled by the harness timeout is exactly the
    // wedged-boot case. The highest-seq VALID snapshot must still decide, and
    // the bad marker must be reported as a count rather than silently dropped.
    // This tail is bracket-closed but missing `ledger_overflow`, so it reaches
    // the shape check.
    let kernel = format!(
        "Added thread 23 'poll_tcp_oracle' to scheduler (user: true, target_cpu: 0)\n{}\n\
         [DISPATCH_STRAND_CENSUS:seq=3:tick=600:ms=3000:saved=13:stranded=1:tids=23:tid_overflow=0]\n",
        marker(2, 400, 2000, 13, 1, "23")
    );
    let fixture = SerialFixture::new(&kernel, "");
    let output = fixture.run(Some("10"));
    let text = output_text(&output);

    assert!(!output.status.success(), "unexpected green verdict: {text}");
    assert!(
        text.contains("thread 23 (poll_tcp_oracle)"),
        "a malformed trailing marker discarded the red reading: {text}"
    );
    assert!(
        text.contains("1 malformed marker(s) skipped"),
        "the malformed marker was not reported as a count: {text}"
    );
}

#[test]
fn a_tail_truncated_mid_marker_does_not_discard_the_red_reading() {
    // #775 round 3, N6, the other truncation shape: the harness kill lands
    // before the closing bracket, so the fragment is not a marker at all. The
    // red reading still has to survive it.
    let kernel = format!(
        "Added thread 23 'poll_tcp_oracle' to scheduler (user: true, target_cpu: 0)\n{}\n\
         [DISPATCH_STRAND_CENSUS:seq=3:tick=600:ms=3000:saved=13:stranded=1:tids=23:tid_overflo",
        marker(2, 400, 2000, 13, 1, "23")
    );
    let fixture = SerialFixture::new(&kernel, "");
    let output = fixture.run(Some("10"));
    let text = output_text(&output);

    assert!(!output.status.success(), "unexpected green verdict: {text}");
    assert!(
        text.contains("thread 23 (poll_tcp_oracle)"),
        "an unterminated trailing fragment discarded the red reading: {text}"
    );
    assert!(
        text.contains("latest snapshot seq=2"),
        "the unterminated fragment was read as a snapshot: {text}"
    );
}

#[test]
fn an_overflowed_ledger_is_never_reported_as_a_clean_census() {
    // #775 round 3, F21: `ledger_overflow>0` means the snapshot is incomplete,
    // so its `stranded=0` is not evidence. The census exits 3 and the verdict
    // says so loudly instead of treating it as a clean reading.
    let kernel = format!(
        "[DISPATCH_STRAND_CENSUS:seq=1:tick=200:ms=1000:saved=11:stranded=0:tids=-:\
         tid_overflow=0:ledger_overflow=7]\n{}",
        tail_pass_with(
            "[DISPATCH_STRAND_CENSUS:seq=2:tick=400:ms=2000:saved=11:stranded=0:tids=-:\
             tid_overflow=0:ledger_overflow=7]"
        )
    );
    let fixture = SerialFixture::new(&kernel, "");
    let output = fixture.run(Some("10"));
    let text = output_text(&output);

    assert!(
        text.contains("STRAND CENSUS INCOMPLETE"),
        "an overflowed ledger was not reported loudly: {text}"
    );
    assert!(
        text.contains("kernel ledger overflowed (7 event(s))"),
        "the overflow count was not reported: {text}"
    );
    assert!(
        !text.contains("STRAND_CENSUS: threads_saved_blocked=11 stranded=0"),
        "an incomplete snapshot printed a clean census summary: {text}"
    );
}

#[test]
fn snapshots_from_two_boots_are_rejected_rather_than_mixed() {
    // #775 round 3, F9: seq is unique within a boot, so a repeated seq with a
    // different payload means more than one boot is in the input.
    let kernel = format!(
        "{}\n{}\n{}",
        marker(1, 200, 1000, 11, 0, "-"),
        marker(1, 205, 1004, 12, 1, "23"),
        // Truncated tail: the two-boot rejection is checked BEFORE the
        // completion-marker arm, so this fixture pins that precedence too.
        tail_pass_truncated()
    );
    let fixture = SerialFixture::new(&kernel, "");
    let output = fixture.run(Some("10"));
    let text = output_text(&output);

    assert!(
        text.contains("more than one boot"),
        "two boots were silently mixed: {text}"
    );
    assert!(
        text.contains("census unavailable"),
        "the two-boot rejection was not treated as census unavailability: {text}"
    );
}

#[test]
fn the_census_reports_its_snapshot_provenance_and_observed_cadence() {
    // #775 round 3, N14: the reading is a snapshot, so the gate transcript has
    // to carry which snapshot it was and how far apart the snapshots landed.
    let kernel = format!(
        "{}\n{}\n{}\n{}",
        marker(1, 200, 1000, 11, 0, "-"),
        marker(2, 400, 2000, 11, 0, "-"),
        marker(3, 900, 4500, 11, 0, "-"),
        tail_pass_with(&marker(4, 1000, 5000, 11, 0, "-"))
    );
    let fixture = SerialFixture::new(&kernel, "");
    let output = fixture.run(Some("10"));
    let text = output_text(&output);

    assert!(
        text.contains("latest snapshot seq=4 tick=1000 at 5000 ms; 4 valid snapshot(s)"),
        "the snapshot judged was not identified: {text}"
    );
    assert!(
        text.contains("previous 500 ms earlier, largest gap 2500 ms"),
        "the observed cadence was not reported: {text}"
    );
}

/// #775 round 4, R3-5/N14. The completion site emits a snapshot right after
/// `USERSPACE TEST COMPLETE`, so a capture that reaches that point carries a
/// kernel timestamp for a known late instant. The gate asserts how stale the
/// newest CADENCE snapshot was at that instant.
fn capture_with_completion_age(cadence_ms: u32, completion_ms: u32) -> String {
    format!(
        "{}\n{}\nUSERSPACE TEST COMPLETE\n{}\nTEST_TALLY: exited=10 nonzero=0 failed=[]\n🏁 TEST RUNNER: All tests passed\n",
        marker(1, 100, 500, 11, 0, "-"),
        marker(2, 200, cadence_ms, 11, 0, "-"),
        marker(3, 300, completion_ms, 11, 0, "-")
    )
}

#[test]
fn a_fresh_census_at_the_completion_marker_passes_and_prints_the_age() {
    let fixture = SerialFixture::new(&capture_with_completion_age(2000, 2100), "");
    let output = fixture.run(Some("10"));
    let text = output_text(&output);

    assert!(output.status.success(), "unexpected red verdict: {text}");
    assert!(
        text.contains("age at the completion marker: 100 ms"),
        "the age was not printed: {text}"
    );
    // The bound is derived, and the tool prints which bound it applied. #766
    // measured the wake-to-dispatch overrun this cadence rides on at max
    // 10318 ms over 324 trials; 15000 ms is that maximum plus margin.
    // claim-lint:ok: the distribution is
    // docs/planning/green-program/sockets/693-RCA-2026-09-02.md lines 109-110.
    assert!(
        text.contains("bound 15000 ms"),
        "the derived bound was not printed with the age: {text}"
    );
}

#[test]
fn a_stale_clean_census_at_the_completion_marker_is_not_a_pass() {
    // Same fixture shape and the same stranded=0; only the cadence gap changes.
    // 19000 ms is over the derived 15000 ms bound, and over #766's measured
    // maximum wake-to-dispatch overrun of 10318 ms as well, so it is not a
    // reading the known latency explains.
    let fixture = SerialFixture::new(&capture_with_completion_age(1500, 20500), "");
    let output = fixture.run(Some("10"));
    let text = output_text(&output);

    assert!(!output.status.success(), "stale census passed: {text}");
    assert!(
        text.contains("age at the completion marker: 19000 ms"),
        "the measured age was not printed: {text}"
    );
    assert!(
        text.contains("stale rather than clean"),
        "the staleness verdict did not reach the gate: {text}"
    );
}

#[test]
fn a_capture_without_a_completion_marker_says_the_age_is_not_measurable() {
    // The zero-feature production profile runs no test runner, so its captures
    // carry no completion marker and no late kernel timestamp. The census must
    // say so rather than invent a reference.
    // claim-lint:ok: 6 of 6 round-4 production captures under
    // docs/planning/green-program/sockets/serials/775/round4/production/
    // carry 0 completion markers.
    let kernel = format!("{}\n{}\n", marker(1, 100, 500, 4, 0, "-"), marker(2, 900, 40000, 6, 0, "-"));
    let fixture = SerialFixture::new(&kernel, "");
    let output = fixture.run(Some("10"));
    let text = output_text(&output);

    assert!(
        text.contains("age at the completion marker: not measurable"),
        "the age line is missing on a capture with no completion marker: {text}"
    );
    assert!(
        !text.contains("stale rather than clean"),
        "an unmeasurable age was reported as staleness: {text}"
    );
    assert!(
        text.contains("USERSPACE TEST COMPLETE was absent"),
        "the unmeasurable age masked the real first cause: {text}"
    );
}

/// The `seq` of a snapshot on this line, if the line carries one.
fn snapshot_seq(line: &str) -> Option<u32> {
    let start = line.find("[DISPATCH_STRAND_CENSUS:seq=")? + "[DISPATCH_STRAND_CENSUS:seq=".len();
    let rest = &line[start..];
    let end = rest.find(':')?;
    rest[..end].parse().ok()
}

/// The one age line of a census run, or a panic naming what was printed.
fn age_line(text: &str) -> String {
    text.lines()
        .find(|line| line.contains("age at the completion marker"))
        .unwrap_or_else(|| panic!("no age line in the census output: {text}"))
        .to_string()
}

/// The committed round-4 gate capture this file's two order tests read.
const GATE_GREEN_BOOT1: &str =
    "docs/planning/green-program/sockets/serials/775/round4/gate-green/boot1";

/// #775 round 5, finding R4-5. A capture that carries `USERSPACE TEST COMPLETE`
/// but no valid snapshot after it is TRUNCATED, not markerless. Before this
/// round the census printed "this capture carries no USERSPACE TEST COMPLETE"
/// on exactly that shape and then skipped the staleness bound -- the one case
/// the bound exists for. The fixture is the reviewer's repro: the committed
/// gate capture with the snapshots from `seq=29` -- the completion snapshot --
/// upwards deleted.
#[test]
fn a_marker_with_no_snapshot_after_it_is_incomplete_rather_than_markerless() {
    let source = committed(GATE_GREEN_BOOT1).join("serial_kernel.txt");
    let full = fs::read_to_string(&source)
        .unwrap_or_else(|error| panic!("read {}: {error}", source.display()));
    let truncated: String = full
        .lines()
        .filter(|line| snapshot_seq(line).is_none_or(|seq| seq < 29))
        .fold(String::new(), |mut text, line| {
            text.push_str(line);
            text.push('\n');
            text
        });
    assert_eq!(
        truncated.matches("USERSPACE TEST COMPLETE").count(),
        1,
        "the truncation removed the completion marker, so the fixture is not the R4-5 shape"
    );

    let fixture = SerialFixture::new(&truncated, "");
    let output = run_census(&[&fixture.kernel_log]);
    let text = output_text(&output);

    assert_eq!(
        output.status.code(),
        Some(2),
        "a truncated capture was not reported as census-unavailable: {text}"
    );
    assert!(
        text.contains("census incomplete at completion marker"),
        "the truncated capture was not named as such: {text}"
    );
    assert!(
        !text.contains("carries no USERSPACE TEST COMPLETE"),
        "a capture that carries the marker was reported as carrying none: {text}"
    );
}

/// #775 round 5, finding R4-6. The age line was computed from a flag streamed
/// over `cat -- "$@"`, so which file the completion marker landed in relative
/// to the snapshots decided the outcome. It is now located by position WITHIN
/// one capture. Both halves are checked: the committed capture, whose whole
/// output must be byte-identical either way, and the split shape the finding
/// used, where the two orders used to disagree about whether the capture had a
/// marker at all.
#[test]
fn the_age_line_does_not_depend_on_argument_order() {
    let kernel = committed(GATE_GREEN_BOOT1).join("serial_kernel.txt");
    let user = committed(GATE_GREEN_BOOT1).join("serial_user.txt");
    let forward = output_text(&run_census(&[&kernel, &user]));
    let reversed = output_text(&run_census(&[&user, &kernel]));

    assert_eq!(
        age_line(&forward),
        age_line(&reversed),
        "the age line moved with the argument order"
    );
    assert_eq!(
        forward, reversed,
        "the census output moved with the argument order"
    );
    // Re-derived by running the tool on the committed capture at write time.
    assert!(
        forward.contains(
            "age at the completion marker: 1137 ms (newest cadence snapshot seq=28 at 49903 ms, \
             completion snapshot seq=29 at 51040 ms, bound 15000 ms)"
        ),
        "the committed capture's age line changed: {forward}"
    );

    // The split shape: the marker in one file, the snapshots in another.
    let marker_only = SerialFixture::new("USERSPACE TEST COMPLETE\n", "");
    let snapshots_only = SerialFixture::new(
        &format!(
            "{}\n{}\n",
            marker(1, 1, 1000, 0, 0, "-"),
            marker(2, 2, 40000, 0, 0, "-")
        ),
        "",
    );
    let marker_first = run_census(&[&marker_only.kernel_log, &snapshots_only.kernel_log]);
    let snapshots_first = run_census(&[&snapshots_only.kernel_log, &marker_only.kernel_log]);
    assert_eq!(
        output_text(&marker_first),
        output_text(&snapshots_first),
        "the split capture read differently in the two argument orders"
    );
    assert_eq!(
        marker_first.status.code(),
        snapshots_first.status.code(),
        "the split capture exited differently in the two argument orders"
    );
}

/// #775 round 5. A census that exits 0 without printing its summary line has
/// not RUN, and the gate used to read that as a clean census. The state is not
/// hypothetical: an apostrophe inside a comment in the single-quoted awk
/// program terminates the program string, and what is left prints nothing and
/// exits 0. This round produced exactly that while editing a comment, and 6 of
/// the 19 tests here stayed green against the broken tool.
#[test]
fn a_census_that_exits_zero_without_a_summary_line_is_not_a_pass() {
    // The verdict script resolves the census by its OWN directory, so the stub
    // has to live next to a copy of it.
    let fixture = SerialFixture::new(&green_log(), "");
    let stub_dir = fixture.directory.join("scripts");
    fs::create_dir(&stub_dir).expect("create stub script directory");
    fs::copy(
        repo_root().join("scripts/x86-gate-verdict.sh"),
        stub_dir.join("x86-gate-verdict.sh"),
    )
    .expect("copy the verdict script beside the stub");
    let stub = stub_dir.join("x86-strand-census.sh");
    fs::write(&stub, "#!/usr/bin/env bash\nexit 0\n").expect("write the silent census stub");
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755))
        .expect("make the silent census stub executable");
    // The verdict script reads its allowlist from the same directory.
    fs::copy(
        repo_root().join("scripts/x86-gate-allowlist.txt"),
        stub_dir.join("x86-gate-allowlist.txt"),
    )
    .expect("copy the allowlist beside the stub");

    let output = Command::new("bash")
        .arg(stub_dir.join("x86-gate-verdict.sh"))
        .arg(&fixture.kernel_log)
        .arg(&fixture.user_log)
        .current_dir(repo_root())
        .env("EXPECTED_EXITS", "10")
        .output()
        .expect("run the copied verdict script");
    let text = output_text(&output);

    assert!(
        !output.status.success(),
        "a census that printed nothing was scored as a pass: {text}"
    );
    assert!(
        text.contains("did not run to completion"),
        "the silent census was not named as the cause: {text}"
    );
}
