//! Behavioral pin for #890: real rustc fixtures, bounded dispatch, and a
//! green -> red -> green mutation under both default and sequential jobs.
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf, RefCell<PathBuf>);

impl Fixture {
    fn new() -> Self {
        static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // Spaces and a quote exercise paths passed through the worker environment.
        let root =
            std::env::temp_dir().join(format!("gsp fixture ' {} {nonce} {id}", std::process::id()));
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/run-structure-tests.sh"),
            root.join("scripts/run-structure-tests.sh"),
        )
        .unwrap();
        Self(root, RefCell::new(PathBuf::new()))
    }

    fn suite(&self, name: &str, fail: bool) {
        fs::write(
            self.0.join(format!("tests/{name}_structure.rs")),
            format!(
                r#"
use std::io::Write;
#[test]
fn {name}_fixture() {{
    let root = std::path::PathBuf::from(std::env::var("FIXTURE_ROOT").unwrap());
    let event = |phase: &str| {{
        let mut file = std::fs::OpenOptions::new().create(true).append(true)
            .open(root.join("events")).unwrap();
        file.write_all(format!("{{phase}} {name}\n").as_bytes()).unwrap();
    }};
    event("start");
    if std::env::var_os("FIXTURE_OVERLAP").is_some() && "{name}" != "charlie" {{
        std::fs::write(root.join("{name}.started"), "").unwrap();
        let other = if "{name}" == "alpha" {{ "bravo" }} else {{ "alpha" }};
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !root.join(format!("{{other}}.started")).exists() {{
            assert!(std::time::Instant::now() < deadline, "parallel worker did not start");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }}
    }}
    event("end");
    assert!(!{fail}, "{name} deliberate fixture failure");
}}
"#
            ),
        )
        .unwrap();
    }

    fn command(&self, jobs: Option<&str>, overlap: bool, gate_tmp: &Path) -> Command {
        fs::write(self.0.join("events"), "").unwrap();
        let mut command = Command::new("/bin/bash");
        command
            .arg("-c")
            .arg("set -euo pipefail; source \"$1\"; gate_structure_preflight \"$2\" \"$3\"")
            .arg("fixture")
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("docker/qemu/lib/gate-structure-preflight.sh"),
            )
            .arg(&self.0)
            .arg(gate_tmp)
            .env("TMPDIR", self.0.join("tmp"))
            .env("FIXTURE_ROOT", &self.0)
            .env_remove("BREENIX_GATE_SKIP_STRUCTURE")
            .env_remove("BREENIX_STRUCTURE_JOBS")
            .env_remove("BREENIX_STRUCTURE_SUITE_TIMEOUT_SECS")
            .env_remove("FIXTURE_OVERLAP");
        if let Some(jobs) = jobs {
            command.env("BREENIX_STRUCTURE_JOBS", jobs);
        }
        if overlap {
            command.env("FIXTURE_OVERLAP", "1");
        }
        command
    }

    fn run(&self, jobs: Option<&str>, overlap: bool) -> Output {
        let gate = self.0.join("gate");
        fs::create_dir_all(&gate).unwrap();
        let before = entries(&gate);
        let output = self.command(jobs, overlap, &gate).output().unwrap();
        let added: Vec<_> = entries(&gate).difference(&before).cloned().collect();
        assert_eq!(added.len(), 1, "one private directory per invocation");
        *self.1.borrow_mut() = added[0].clone();
        output
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.1.borrow().join(format!("{name}_structure.log"))).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn mutation_is_green_red_green_with_default_and_one_job() {
    for jobs in [None, Some("1")] {
        let fixture = Fixture::new();
        // Deliberately create these out of sorted order.
        for name in ["charlie", "alpha", "bravo"] {
            fixture.suite(name, false);
        }
        for fail in [false, true, false] {
            fixture.suite("bravo", fail);
            let output = fixture.run(jobs, false);
            let stdout = verdict(&output.stdout);
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert_eq!(output.status.success(), !fail, "{stdout}\n{stderr}");
            assert_eq!(
                stdout,
                format!(
                    "[GATE_PREFLIGHT:structure_suites={}/3:critical_path_lines=0:pinned=0]\n",
                    if fail { 2 } else { 3 }
                )
            );
            for name in ["alpha", "bravo", "charlie"] {
                let log = fixture.log(name);
                assert!(
                    log.contains(&format!(
                        "test {name}_fixture ... {}",
                        if fail && name == "bravo" {
                            "FAILED"
                        } else {
                            "ok"
                        }
                    )),
                    "{log}"
                );
                assert_eq!(
                    log.contains("bravo deliberate fixture failure"),
                    fail && name == "bravo"
                );
            }
            if fail {
                assert_eq!(stderr, format!("GATE_PREFLIGHT: FAIL (1 of 3 structure suite(s) red: bravo_structure -- per-suite logs under {})\n", fixture.1.borrow().display()));
            } else {
                assert!(stderr.is_empty(), "{stderr}");
            }
            if jobs == Some("1") {
                assert_eq!(
                    fs::read_to_string(fixture.0.join("events")).unwrap(),
                    "start alpha\nend alpha\nstart bravo\nend bravo\nstart charlie\nend charlie\n"
                );
            }
        }
    }
}

#[test]
fn two_jobs_overlap_and_never_exceed_the_bound() {
    let fixture = Fixture::new();
    for name in ["charlie", "bravo", "alpha"] {
        fixture.suite(name, false);
    }
    let output = fixture.run(Some("2"), true);
    assert!(
        output.status.success(),
        "{}\n{}",
        verdict(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut active = 0;
    let mut peak = 0;
    for line in fs::read_to_string(fixture.0.join("events"))
        .unwrap()
        .lines()
    {
        if line.starts_with("start ") {
            active += 1;
        } else {
            active -= 1;
        }
        assert!((0..=2).contains(&active));
        peak = peak.max(active);
    }
    assert_eq!(active, 0);
    assert_eq!(peak, 2);
}

fn entries(path: &Path) -> BTreeSet<PathBuf> {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

#[test]
fn concurrent_invocations_isolate_logs_and_same_stem_binaries() {
    let first = Fixture::new();
    let second = Fixture::new();
    let shared = Fixture::new(); // independent fresh directory shared by both calls
    for (fixture, peer) in [(&first, &second), (&second, &first)] {
        fs::write(
            fixture.0.join("tests/alpha_structure.rs"),
            format!(
                r#"
#[test]
fn alpha_fixture() {{
    assert_eq!(env!("CARGO_MANIFEST_DIR"), std::env::var("FIXTURE_ROOT").unwrap());
    let tmp = std::env::var("TMPDIR").unwrap();
    std::fs::write({ready:?}, &tmp).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !std::path::Path::new({peer:?}).exists() {{
        assert!(std::time::Instant::now() < deadline, "peer did not execute");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }}
    let other = std::fs::read_to_string({peer:?}).unwrap();
    assert_ne!(tmp, other, "runner binary namespace must be private");
}}
"#,
                ready = fixture.0.join("ready"),
                peer = peer.0.join("ready")
            ),
        )
        .unwrap();
    }
    let mut a = first.command(Some("1"), false, &shared.0);
    let mut b = second.command(Some("1"), false, &shared.0);
    for command in [&mut a, &mut b] {
        command
            .env("TMPDIR", &shared.0)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
    }
    let a = a.spawn().unwrap();
    let b = b.spawn().unwrap();
    for output in [a.wait_with_output().unwrap(), b.wait_with_output().unwrap()] {
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            verdict(&output.stdout),
            "[GATE_PREFLIGHT:structure_suites=1/1:critical_path_lines=0:pinned=0]\n"
        );
        assert!(output.stderr.is_empty(), "{output:?}");
    }
    let dirs: Vec<_> = entries(&shared.0)
        .into_iter()
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("breenix_gate_structure_preflight.")
        })
        .collect();
    assert_eq!(dirs.len(), 2);
    for dir in dirs {
        assert!(dir.join("alpha_structure.log").is_file());
        assert!(dir
            .join("breenix-structure-tests/alpha_structure")
            .is_file());
    }
}

#[test]
fn hung_suite_returns_red_within_budget() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("tests/hung_structure.rs"),
        "#[test] fn hangs() { loop { std::thread::sleep(std::time::Duration::from_millis(100)); } }").unwrap();
    let start = std::time::Instant::now();
    let output = fixture
        .command(Some("1"), false, &fixture.0.join("gate"))
        .env("BREENIX_STRUCTURE_SUITE_TIMEOUT_SECS", "2")
        .output()
        .unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(20));
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        verdict(&output.stdout),
        "[GATE_PREFLIGHT:structure_suites=0/1:critical_path_lines=0:pinned=0]\n"
    );
}

#[test]
fn oversized_jobs_are_configuration_errors() {
    let fixture = Fixture::new();
    fixture.suite("alpha", false);
    for jobs in ["2147483648", "999999999999999999999999999999999999"] {
        let output = fixture
            .command(Some(jobs), false, &fixture.0.join("gate"))
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{stderr}");
        assert!(
            stderr.contains("invalid BREENIX_STRUCTURE_JOBS"),
            "{stderr}"
        );
        assert!(!stderr.contains("suite(s) red"), "{stderr}");
        assert!(!stderr.contains("xargs:"), "{stderr}");
    }
}

fn verdict(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter(|line| !line.starts_with("[GATE_SUITE"))
        .map(|line| format!("{line}\n"))
        .collect()
}

#[test]
fn timeout_retries_only_that_suite_once_with_double_budget() {
    for (first, second, success, attempts) in
        [(124, 0, true, 2), (124, 124, false, 2), (1, 0, false, 1)]
    {
        let fixture = Fixture::new();
        fixture.suite("alpha", false);
        fixture.suite("bravo", false);
        let bin = fixture.0.join("bin");
        fs::create_dir(&bin).unwrap();
        let timeout = bin.join("timeout");
        fs::write(
            &timeout,
            format!(
                r#"#!/bin/bash
stem="$4"
echo "$stem $1" >> "$FIXTURE_ROOT/attempts"
if [ "$stem" = alpha_structure ]; then
    if [ ! -e "$FIXTURE_ROOT/retried" ]; then
        touch "$FIXTURE_ROOT/retried"
        exit {first}
    fi
    exit {second}
fi
exit 0
"#
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&timeout, fs::Permissions::from_mode(0o755)).unwrap();
        let output = fixture
            .command(Some("1"), false, &fixture.0.join("gate"))
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("BREENIX_STRUCTURE_SUITE_TIMEOUT_SECS", "7")
            .output()
            .unwrap();
        assert_eq!(output.status.success(), success, "{output:?}");
        let calls = fs::read_to_string(fixture.0.join("attempts")).unwrap();
        assert_eq!(
            calls,
            if attempts == 2 {
                "alpha_structure 7\nalpha_structure 14\nbravo_structure 7\n"
            } else {
                "alpha_structure 7\nbravo_structure 7\n"
            }
        );
        let out = String::from_utf8_lossy(&output.stdout);
        assert_eq!(out.contains("host_load="), first == 124);
        assert!(out.contains("wall_s="));
    }
}
