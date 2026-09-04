use std::fs;
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

/// One snapshot in the shape `report_snapshot()` emits.
fn marker(seq: u32, tick: u32, ms: u32, saved: u32, stranded: u32, tids: &str) -> String {
    format!(
        "[DISPATCH_STRAND_CENSUS:seq={seq}:tick={tick}:ms={ms}:saved={saved}:\
         stranded={stranded}:tids={tids}:tid_overflow=0:ledger_overflow=0]"
    )
}

fn tail_pass() -> &'static str {
    "USERSPACE TEST COMPLETE\n\
     TEST_TALLY: exited=10 nonzero=0 failed=[]\n\
     🏁 TEST RUNNER: All tests passed\n"
}

fn green_log() -> String {
    format!("{}\n{}", marker(1, 200, 1000, 11, 0, "-"), tail_pass())
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
            "{}\nUSERSPACE TEST COMPLETE\n\
             TEST_TALLY: exited=10 nonzero=1 failed=[brk_test:-11]\n\
             TEST RUNNER: FAILED\n",
            marker(1, 200, 1000, 11, 0, "-")
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
    let kernel = format!(
        "{} {}\n{}",
        marker(1, 200, 1000, 13, 1, "23"),
        marker(2, 400, 2000, 13, 0, "-"),
        tail_pass()
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
        tail_pass()
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
        tail_pass()
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
        tail_pass()
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
        tail_pass()
    );
    let fixture = SerialFixture::new(&kernel, "");
    let output = fixture.run(Some("10"));
    let text = output_text(&output);

    assert!(
        text.contains("latest snapshot seq=3 tick=900 at 4500 ms; 3 valid snapshot(s)"),
        "the snapshot judged was not identified: {text}"
    );
    assert!(
        text.contains("previous 2500 ms earlier, largest gap 2500 ms"),
        "the observed cadence was not reported: {text}"
    );
}
