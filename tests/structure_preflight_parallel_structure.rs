//! Behavioral pin for #890: real rustc fixtures, bounded dispatch, and a
//! green -> red -> green mutation under both default and sequential jobs.
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

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
        Self(root)
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

    fn run(&self, jobs: Option<&str>, overlap: bool) -> Output {
        fs::write(self.0.join("events"), "").unwrap();
        let mut command = Command::new("/bin/bash");
        command
            .arg("-c")
            .arg("set -euo pipefail; source \"$1\"; gate_structure_preflight \"$2\" \"$2/gate\"")
            .arg("fixture")
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("docker/qemu/lib/gate-structure-preflight.sh"),
            )
            .arg(&self.0)
            .env("TMPDIR", self.0.join("tmp"))
            .env("FIXTURE_ROOT", &self.0)
            .env_remove("BREENIX_GATE_SKIP_STRUCTURE")
            .env_remove("BREENIX_STRUCTURE_JOBS")
            .env_remove("FIXTURE_OVERLAP");
        if let Some(jobs) = jobs {
            command.env("BREENIX_STRUCTURE_JOBS", jobs);
        }
        if overlap {
            command.env("FIXTURE_OVERLAP", "1");
        }
        command.output().unwrap()
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.0.join(format!(
            "gate/breenix_gate_structure_preflight/{name}_structure.log"
        )))
        .unwrap()
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
            let stdout = String::from_utf8(output.stdout).unwrap();
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
                assert_eq!(stderr, format!("GATE_PREFLIGHT: FAIL (1 of 3 structure suite(s) red: bravo_structure -- per-suite logs under {}/gate/breenix_gate_structure_preflight)\n", fixture.0.display()));
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
        String::from_utf8_lossy(&output.stdout),
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
