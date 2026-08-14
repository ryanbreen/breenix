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
        let mut command = Command::new("bash");
        command
            .arg(repo_root().join("scripts/x86-gate-verdict.sh"))
            .arg(&self.kernel_log)
            .arg(&self.user_log)
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

fn green_log() -> &'static str {
    "USERSPACE TEST COMPLETE\n\
     TEST_TALLY: exited=10 nonzero=0 failed=[]\n\
     🏁 TEST RUNNER: All tests passed\n"
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
    let fixture = SerialFixture::new(green_log(), "");
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
    let fixture = SerialFixture::new(green_log(), "");
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
    let fixture = SerialFixture::new(green_log(), "");
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
    let fixture = SerialFixture::new(green_log(), "");

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
        "USERSPACE TEST COMPLETE\n\
         TEST_TALLY: exited=10 nonzero=1 failed=[brk_test:-11]\n\
         TEST RUNNER: FAILED\n",
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
