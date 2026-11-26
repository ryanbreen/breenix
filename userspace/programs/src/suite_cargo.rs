//! Exercise the hosted Cargo and rustc against the K7 scratch fixture.
use breenix_suite::suite::{case, case_ms_left, category, suite, CaseError, CaseResult, Suite};
use libbreenix::process::{self, ForkResult, WNOHANG};
use libbreenix::signal::{self, SIGKILL};
use serde_json::Value;
use std::fs;
use std::io;
use std::os::unix::{fs::DirBuilderExt, process::ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

static SUITE: Suite = suite(
    "cargo",
    "Cargo",
    &[category(
        "cargo",
        "Offline Cargo",
        &[
            case(
                "offline-path-workspace",
                "Builds and runs a path workspace",
                offline_path_workspace,
            ),
            case(
                "no-network",
                "Refuses an uncached registry dependency",
                no_network,
            ),
        ],
    )],
)
.case_limit_ms(60_000);

const FIXTURE: &str = "/usr/libexec/breenix/rustc";
const CORPUS: &str = "/usr/libexec/breenix/cargo/corpus";

// tests/cargo_suite_test.py projects these commands onto the Mac twin fixture.
const INVOCATIONS: &str = r#"{
    "offline-path-workspace": {
        "build": {
            "argv": [
                "{fixture}/bin/cargo", "build", "--offline", "--frozen",
                "--color", "never", "--jobs", "1", "--release",
                "--target", "{fixture}/targets/x86_64-breenix.json",
                "--target-dir", "target"
            ],
            "cwd": "{workspace}",
            "env": {
                "HOME": "{workspace}",
                "CARGO_HOME": "{workspace}/cargo-home",
                "RUSTC": "{fixture}/bin/rustc",
                "RUST_TARGET_PATH": "{fixture}/targets",
                "CARGO_ENCODED_RUSTFLAGS": "--sysroot={fixture}/sysroot\u001f-Clinker={fixture}/bin/bld\u001f-Clink-arg=-T{fixture}/lib/linker.ld\u001f-Lnative={fixture}/lib\u001f-Clink-arg=--allow-multiple-definition\u001f-Cdefault-linker-libraries=no\u001f--remap-path-prefix={workspace}=/breenix-k7\u001f-Dwarnings"
            }
        },
        "run": {
            "argv": ["{workspace}/target/x86_64-breenix/release/k7-app"],
            "cwd": "{workspace}",
            "env": {}
        }
    },
    "no-network": {
        "build": {
            "argv": ["{fixture}/bin/cargo", "fetch", "--offline", "--frozen", "--color", "never"],
            "cwd": "{workspace}",
            "env": {
                "HOME": "{workspace}",
                "CARGO_HOME": "{workspace}/cargo-home",
                "RUSTC": "{fixture}/bin/rustc",
                "RUST_TARGET_PATH": "{fixture}/targets",
                "CARGO_NET_RETRY": "0"
            }
        }
    }
}"#;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let output = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            fs::create_dir(&output)?;
            copy_tree(&entry.path(), &output)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), output)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "non-file corpus entry",
            ));
        }
    }
    Ok(())
}

fn read_expectation() -> Result<Value, String> {
    let path = Path::new(CORPUS).join("expect.json");
    let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn run_step(step: &Value, expected: &Value, workspace: &Path) -> Result<(), String> {
    let expand = |value: &Value| {
        value
            .as_str()
            .map(|value| {
                value
                    .replace("{fixture}", FIXTURE)
                    .replace("{workspace}", &workspace.to_string_lossy())
            })
            .ok_or_else(|| format!("invalid spawn argument: {value}"))
    };
    let argv = step["argv"].as_array().ok_or("missing argv")?;
    let (program, args) = argv.split_first().ok_or("empty argv")?;
    let program = expand(program)?;
    let mut command = Command::new(&program);
    command
        .args(args.iter().map(expand).collect::<Result<Vec<_>, _>>()?)
        .current_dir(expand(&step["cwd"])?)
        .env_clear()
        .stdin(Stdio::null());
    for (key, value) in step["env"].as_object().ok_or("missing environment")? {
        command.env(key, expand(value)?);
    }
    let output = command
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    let status = expected["status"]
        .as_i64()
        .ok_or("missing expected exit status")?;
    if output.status.code().map(i64::from) != Some(status) {
        return Err(format!(
            "{program}: expected exit {status}, got {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    for (stream, actual) in [("stdout", output.stdout), ("stderr", output.stderr)] {
        if let Some(value) = expected[stream].as_str() {
            if actual
                != value
                    .replace("{workspace}", &workspace.to_string_lossy())
                    .as_bytes()
            {
                return Err(format!(
                    "{program}: {stream} mismatch: {:?}",
                    String::from_utf8_lossy(&actual)
                ));
            }
        }
    }
    Ok(())
}

fn run_workload(name: &str, workspace: &Path) -> Result<(), String> {
    let source = Path::new(CORPUS).join(name);
    copy_tree(&source, workspace).map_err(|error| format!("{}: {error}", source.display()))?;
    let plan: Value = serde_json::from_str(INVOCATIONS).map_err(|error| error.to_string())?;
    let expected = read_expectation()?;
    run_step(&plan[name]["build"], &expected[name]["build"], workspace)?;
    if name == "offline-path-workspace" {
        return run_step(&plan[name]["run"], &expected[name]["run"], workspace);
    }

    let registry = workspace.join("cargo-home/registry");
    let entries = match fs::read_dir(&registry) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("{}: {error}", registry.display())),
    };
    let metadata = &expected[name]["registry_metadata"];
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", registry.display()))?;
        let filename = entry.file_name();
        let tag = metadata[filename.to_string_lossy().as_ref()]
            .as_str()
            .ok_or_else(|| format!("Cargo populated the offline registry cache: {filename:?}"))?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if !kind.is_file()
            || fs::read(entry.path()).map_err(|error| error.to_string())? != tag.as_bytes()
        {
            return Err(format!("invalid registry metadata: {filename:?}"));
        }
    }
    Ok(())
}

fn run_in_group(name: &str) -> CaseResult {
    let path = PathBuf::from(format!("/tmp/cargo-{name}-{}", std::process::id()));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let scratch = Scratch(path);
    let report = scratch.0.join("failure");
    let pid = match process::fork()? {
        ForkResult::Child => {
            let result = process::setpgid(0, 0)
                .map_err(|error| format!("create Cargo process group: {error}"))
                .and_then(|()| run_workload(name, &scratch.0));
            if let Err(error) = result {
                let _ = fs::write(&report, error);
                process::exit(1);
            }
            process::exit(0);
        }
        ForkResult::Parent(pid) => pid.raw() as i32,
    };

    let mut status = 0;
    loop {
        if process::waitpid(pid, &mut status, WNOHANG)?.raw() != 0 {
            let status = ExitStatus::from_raw(status);
            return if status.success() {
                Ok(())
            } else {
                Err(fs::read_to_string(&report)
                    .unwrap_or_else(|_| format!("Cargo case worker: {status}"))
                    .into())
            };
        }
        // Leave time to kill and reap the group before the suite watchdog.
        if case_ms_left() < 500 {
            signal::kill(-pid, SIGKILL)
                .map_err(|error| format!("timed out, group kill failed: {error}"))?;
            process::waitpid(pid, &mut status, 0)?;
            return Err(CaseError::Fail(
                "timed out and killed the Cargo process group".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn offline_path_workspace() -> CaseResult {
    run_in_group("offline-path-workspace")
}

fn no_network() -> CaseResult {
    run_in_group("no-network")
}

fn main() {
    SUITE.run()
}
