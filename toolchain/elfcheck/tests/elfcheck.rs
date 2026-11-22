//! Exercise the bounds checks over deterministic truncations and corruptions.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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
}

fn write_fixtures(root: &Path, directory: &Path) -> Vec<Case> {
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
    text.lines()
        .map(|line| {
            let mut fields = line.split('\t');
            let mode = match fields.next() {
                Some("relocatable") => Mode::Relocatable,
                Some("exec") => Mode::Exec,
                _ => panic!("unknown fixture mode in {line:?}"),
            };
            let name = fields.next().expect("fixture name");
            let path = fields.next().expect("fixture path");
            assert!(fields.next().is_none(), "unexpected fixture line {line:?}");
            Case {
                mode,
                name: name.into(),
                path: path.into(),
            }
        })
        .collect()
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
    let cases = write_fixtures(&root, &scratch.0.join("fixtures"));
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

    for case in &cases {
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
