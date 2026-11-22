//! `elfcheck::verdicts_equal_c_elfcheck` (plan step 25.R, unit R1).
//!
//! During 25.R the C elfcheck in toolchain/tests/elfcheck is a parity oracle
//! for the Rust one (plan 25.R, decision O9). The test runs each positive
//! fixture (the two baselines) and each negative control that fixtures.py and
//! fixtures_exec.py define through both, in the mode its module names, and
//! requires the same exit status and the same set of named checks from each.
//! It also requires both to agree with the fixture's own expectation, so the
//! two implementations agreeing on a wrong answer fails it.
//!
//! The C oracle is compiled here with the host C compiler (`$CC`, default
//! `cc`) under the strict flags toolchain/tests/elfcheck/run.py uses; python3
//! writes the fixtures through write_fixtures.py.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const STRICT: [&str; 5] = ["-std=c11", "-Wall", "-Wextra", "-Werror", "-pedantic"];

fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .join("../..")
        .canonicalize()
        .expect("the repository root above toolchain/elfcheck exists")
}

/// A scratch directory removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = env::temp_dir().join(format!("elfcheck-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("create the scratch directory");
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Relocatable,
    Exec,
}

struct Case {
    mode: Mode,
    name: String,
    path: String,
    /// Exit status and check ids the fixture module says elfcheck must give.
    expected_status: i32,
    expected_ids: BTreeSet<String>,
}

struct Manifest {
    checks: Vec<String>,
    cases: Vec<Case>,
}

fn write_fixtures(root: &Path, directory: &Path) -> Manifest {
    let script = root.join("toolchain/tests/elfcheck/write_fixtures.py");
    let output = Command::new("python3")
        .arg(&script)
        .arg(directory)
        .output()
        .expect("run python3 toolchain/tests/elfcheck/write_fixtures.py");
    assert!(
        output.status.success(),
        "write_fixtures.py failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("the manifest is UTF-8");
    let mut manifest = Manifest {
        checks: Vec::new(),
        cases: Vec::new(),
    };
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let mode = |field: &str| match field {
            "relocatable" => Mode::Relocatable,
            "exec" => Mode::Exec,
            other => panic!("unknown fixture mode {other:?} in {line:?}"),
        };
        match fields.as_slice() {
            ["check", id] => manifest.checks.push((*id).to_string()),
            ["baseline", m, path] => manifest.cases.push(Case {
                mode: mode(m),
                name: "baseline".to_string(),
                path: (*path).to_string(),
                expected_status: 0,
                expected_ids: BTreeSet::new(),
            }),
            ["control", m, name, path, ids] => manifest.cases.push(Case {
                mode: mode(m),
                name: (*name).to_string(),
                path: (*path).to_string(),
                expected_status: 1,
                expected_ids: ids
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
            }),
            _ => panic!("unexpected manifest line {line:?}"),
        }
    }
    manifest
}

fn build_c_elfcheck(root: &Path, scratch: &Path) -> PathBuf {
    let compiler = env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let output_path = scratch.join("elfcheck-c");
    let output = Command::new(&compiler)
        .args(STRICT)
        .arg("-O2")
        .arg("-o")
        .arg(&output_path)
        .arg(root.join("toolchain/tests/elfcheck/elfcheck.c"))
        .output()
        .unwrap_or_else(|e| panic!("run the C compiler {compiler:?}: {e}"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stderr.trim().is_empty(),
        "the C elfcheck does not build cleanly with {compiler:?}: {stderr}"
    );
    output_path
}

#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    status: i32,
    ids: BTreeSet<String>,
}

struct Run {
    verdict: Verdict,
    stdout: String,
    stderr: String,
}

fn run(binary: &Path, args: &[&str]) -> Run {
    let output = Command::new(binary)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run {}: {e}", binary.display()));
    // A signal (a crash) has no exit code; it is scored as -1, which no fixture expects.
    let status = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let file = args.last().copied().unwrap_or("");
    let prefix = format!("{file}: ");
    let ids = stdout
        .lines()
        .filter_map(|line| line.strip_prefix(&prefix))
        .filter(|rest| *rest != "ok")
        .map(|rest| rest.split_once(": ").map_or(rest, |(id, _)| id).to_string())
        .collect();
    Run {
        verdict: Verdict { status, ids },
        stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Plan section 2: malformed input is a located error, not a panic.
///
/// Each fixture file is truncated (at each length for the two baselines, at
/// each 16th length for the controls), and each baseline is corrupted at each
/// byte (inverted) and at each 4- and 8-aligned offset (a field of 0xff bytes),
/// and checked in-process. A panic fails the test, and so does a
/// `ReadError`: that marks a read elfcheck did not bound first, which is a
/// defect in elfcheck, not in the input. A file shorter than the ELF header
/// must report `ehdr.size`.
#[test]
fn malformed_input_is_error_not_panic() {
    use elfcheck::{Check, Kind, Report};
    use std::panic::{catch_unwind, AssertUnwindSafe};

    let root = repo_root();
    let scratch = Scratch::new("malformed");
    let manifest = write_fixtures(&root, &scratch.0.join("fixtures"));
    let mut problems = Vec::new();
    let mut inputs = 0u64;

    let mut probe = |label: &str, what: String, data: &[u8], kind: Kind| {
        inputs += 1;
        match catch_unwind(AssertUnwindSafe(|| elfcheck::check(data, kind))) {
            Err(_) => problems.push(format!("{label}: {what}: panicked")),
            Ok(Report {
                error: Some(error), ..
            }) => problems.push(format!("{label}: {what}: {error}")),
            Ok(report) => {
                if data.len() < 64 && !report.findings.iter().any(|f| f.check == Check::EhdrSize) {
                    problems.push(format!("{label}: {what}: no ehdr.size finding"));
                }
            }
        }
    };

    for case in &manifest.cases {
        let data = fs::read(&case.path).unwrap_or_else(|e| panic!("read {}: {e}", case.path));
        let kind = match case.mode {
            Mode::Relocatable => Kind::Relocatable,
            Mode::Exec => Kind::Executable,
        };
        let label = format!("{:?} {}", case.mode, case.name);
        let baseline = case.name == "baseline";
        for length in (0..data.len()).step_by(if baseline { 1 } else { 16 }) {
            probe(
                &label,
                format!("truncated to {length} bytes"),
                &data[..length],
                kind,
            );
        }
        if !baseline {
            continue;
        }
        for at in 0..data.len() {
            let mut bad = data.clone();
            bad[at] ^= 0xff;
            probe(&label, format!("byte {at} inverted"), &bad, kind);
            for width in [4, 8] {
                if at % width == 0 && at + width <= data.len() {
                    let mut bad = data.clone();
                    bad[at..at + width].fill(0xff);
                    probe(
                        &label,
                        format!("{width} bytes at {at} set to 0xff"),
                        &bad,
                        kind,
                    );
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{} of {inputs} malformed inputs:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn verdicts_equal_c_elfcheck() {
    let root = repo_root();
    let scratch = Scratch::new("parity");
    let c = build_c_elfcheck(&root, &scratch.0);
    let rust = PathBuf::from(env!("CARGO_BIN_EXE_elfcheck"));
    let manifest = write_fixtures(&root, &scratch.0.join("fixtures"));
    let mut problems = Vec::new();

    let relocatable = manifest
        .cases
        .iter()
        .filter(|c| c.mode == Mode::Relocatable)
        .count();
    let exec = manifest
        .cases
        .iter()
        .filter(|c| c.mode == Mode::Exec)
        .count();
    assert!(
        relocatable > 1 && exec > 1,
        "the fixture modules wrote {relocatable} and {exec} files"
    );

    let listed = |binary: &Path| -> BTreeSet<String> {
        let out = Command::new(binary)
            .arg("--list")
            .output()
            .expect("run --list");
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .map(str::to_string)
            .collect()
    };
    let declared: BTreeSet<String> = manifest.checks.iter().cloned().collect();
    let (c_list, rust_list) = (listed(&c), listed(&rust));
    if rust_list != c_list || rust_list != declared {
        problems.push(format!(
            "--list: Rust {} ids, C {} ids, fixtures {}; Rust lacks {:?}, Rust adds {:?}",
            rust_list.len(),
            c_list.len(),
            declared.len(),
            declared.difference(&rust_list).collect::<Vec<_>>(),
            rust_list.difference(&declared).collect::<Vec<_>>()
        ));
    }

    for case in &manifest.cases {
        let args: Vec<&str> = match case.mode {
            Mode::Relocatable => vec![case.path.as_str()],
            Mode::Exec => vec!["--exec", case.path.as_str()],
        };
        let (c_run, rust_run) = (run(&c, &args), run(&rust, &args));
        let expected = Verdict {
            status: case.expected_status,
            ids: case.expected_ids.clone(),
        };
        let label = format!("{:?} {}", case.mode, case.name);
        if rust_run.verdict != c_run.verdict {
            problems.push(format!(
                "{label}: Rust {:?} but C {:?}\n  Rust stdout: {}\n  Rust stderr: {}",
                rust_run.verdict,
                c_run.verdict,
                rust_run.stdout.trim(),
                rust_run.stderr.trim()
            ));
        }
        if c_run.verdict != expected {
            problems.push(format!(
                "{label}: the C oracle gives {:?}, the fixture expects {expected:?}",
                c_run.verdict
            ));
        } else if rust_run.verdict != expected {
            problems.push(format!(
                "{label}: Rust gives {:?}, the fixture expects {expected:?}",
                rust_run.verdict
            ));
        }
    }

    let missing = scratch.0.join("absent.o");
    let missing = missing.to_string_lossy();
    for (who, binary) in [("C", &c), ("Rust", &rust)] {
        let r = run(binary, &[&missing]);
        if r.verdict.status != 2 || !r.stderr.contains(missing.as_ref()) {
            problems.push(format!(
                "{who}: an unreadable file gives exit {} and stderr {:?}, not exit 2 naming the file",
                r.verdict.status,
                r.stderr.trim()
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "{} of {} fixture files ({} relocatable, {} exec) disagree:\n{}",
        problems.len(),
        manifest.cases.len(),
        relocatable,
        exec,
        problems.join("\n")
    );
}
