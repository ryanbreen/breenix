//! E25 25.8 builds and checks the Rust toolchain on the x86-64 fixture.
#[path = "suite_support/d1.rs"]
mod d1;
#[path = "suite_support/self_host_behave.rs"]
mod self_host_behave;
#[path = "suite_support/self_host_chain.rs"]
mod self_host_chain;

use breenix_suite::suite::{
    case, case_diagnostic, category, running_case, suite, CaseError, CaseResult, Suite,
};
use libbreenix::crypto::{rand::getrandom_bytes, sha256};
use libbreenix::fs as sysfs;
use self_host_chain::{snapshot, verify, verify_noop, Digests, Snapshot, PROFILE, TOOLS};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

const SOURCE: &str = "/src";
const FIXTURE: &str = "/usr/libexec/breenix/rustc";
const LINKER: &str = "/opt/breenix-dev/25.2/bin/ld";
const MARKER: &str = "/src/breenix-self-host";
const MAC_REFERENCE: &str = include_str!("../../../toolchain/rust-host/selfhost/x86_64-mac.json");
const MAC_MATCH: &str = "tools-equal-mac.sha256.json";
const BUILDS: [(&str, u64); 7] = [
    // Ruling 133(2) gives the D1 wall times. Round 1.5x up to milliseconds.
    ("bobj", 17_235),
    ("bas", 34_158),
    ("bld", 60_002),
    ("bar", 15_170),
    ("bnm", 14_211),
    ("bmake", 86_702),
    ("bcc", 660_000), // Ruling 136(2).
];
static RUN: OnceLock<Result<PathBuf, String>> = OnceLock::new();

static SUITE: Suite = suite(
    "self-host",
    "Native Rust toolchain rebuild",
    &[category(
        "self-host",
        "Self-hosted toolchain",
        &[
            case("build-bobj", "Build bobj", build_package),
            case("build-bas", "Build bas", build_package),
            case("build-bld", "Build bld", build_package),
            case("build-bar", "Build bar", build_package),
            case("build-bnm", "Build bnm", build_package),
            case("build-bmake", "Build bmake", build_package),
            case("build-bcc", "Build bcc", build_package),
            case(
                "tools-equal-mac",
                "Match the Mac twin binaries",
                tools_equal_mac,
            ),
            case(
                "rebuilt-tools-behave",
                "Run the corpus with the rebuilt tools",
                rebuilt_tools_behave,
            ),
            case(
                "rebuilt-cargo-noop",
                "Repeat Cargo build without recompiling",
                rebuilt_cargo_noop,
            ),
        ],
    )],
)
.with_case_observer(prepare_scratch)
.case_limit_ms(670_000);

fn prepare_scratch(completed: usize, _: usize, _: &str) {
    if completed == 0 {
        let _ = RUN.set(mount_fixture());
    }
}

fn mount_fixture() -> Result<PathBuf, String> {
    if !cfg!(target_arch = "x86_64") {
        return Err("The self-host fixture targets x86-64".into());
    }
    if std::env::args_os().any(|arg| arg == "--case") {
        return Err("The self-host chain requires an explicit full-suite run".into());
    }
    fs::create_dir_all(SOURCE).map_err(|error| format!("create {SOURCE}: {error}"))?;
    let mut fixtures = Vec::new();
    for entry in fs::read_dir("/dev").map_err(|error| format!("list /dev: {error}"))? {
        let path = entry
            .map_err(|error| format!("read /dev entry: {error}"))?
            .path();
        let Some(device) = path.to_str() else {
            continue;
        };
        if device
            .strip_prefix("/dev/disk")
            .and_then(|n| n.parse::<u8>().ok())
            .is_none()
        {
            continue;
        }
        // SAFETY: the device and mountpoint are valid paths and BreenFS takes no mount data.
        if unsafe { sysfs::mount(device, SOURCE, "breenfs", 1, std::ptr::null()) }.is_err() {
            continue;
        }
        let marked = fs::read(MARKER).is_ok_and(|bytes| bytes == b"e25-self-host\n");
        sysfs::umount2(SOURCE, 0).map_err(|error| format!("detach {device}: {error}"))?;
        if marked {
            fixtures.push(path);
        }
    }
    let [device] = fixtures.as_slice() else {
        return Err(format!(
            "expected one self-host scratch fixture, found {fixtures:?}"
        ));
    };
    let device = device.to_str().ok_or("invalid self-host device path")?;
    // SAFETY: this is the uniquely marked device found by the read-only probe.
    unsafe { sysfs::mount(device, SOURCE, "breenfs", 0, std::ptr::null()) }
        .map_err(|error| format!("mount {device} at {SOURCE}: {error}"))?;
    let setup = (|| {
        let mut nonce = [0; 16];
        if getrandom_bytes(&mut nonce) != Ok(nonce.len()) {
            return Err("could not generate the self-host boot nonce".into());
        }
        let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
        let run = Path::new(SOURCE).join(nonce);
        fs::create_dir(&run).map_err(|error| format!("create self-host run directory: {error}"))?;
        for name in ["target", "tmp", "cargo-home/registry"] {
            fs::create_dir_all(run.join(name))
                .map_err(|error| format!("create self-host {name}: {error}"))?;
        }
        Ok(run)
    })();
    if setup.is_err() {
        sysfs::umount2(SOURCE, 0).map_err(|error| format!("detach self-host fixture: {error}"))?;
    }
    setup
}

fn run_dir() -> Result<&'static Path, CaseError> {
    RUN.get()
        .ok_or("missing self-host fixture setup")?
        .as_deref()
        .map_err(|error| CaseError::Fail(error.clone()))
}

fn artifact_snapshot(run: &Path) -> Result<Snapshot, CaseError> {
    snapshot(&run.join("target")).map_err(|error| format!("read Cargo outputs: {error}").into())
}

fn verify_record(run: &Path, package: &str, current: &Snapshot) -> CaseResult {
    let record = fs::read(run.join(format!("{package}.sha256.json")))
        .map_err(|error| format!("read {package} digest record: {error}"))?;
    let recorded: Digests = serde_json::from_slice(&record)
        .map_err(|error| format!("parse {package} digest record: {error}"))?;
    verify(&recorded, current).map_err(Into::into)
}

fn cargo(run: &Path, package: &str) -> Command {
    let fixture = Path::new(FIXTURE);
    d1::cargo(
        Path::new(SOURCE),
        fixture,
        Path::new(LINKER),
        run,
        package,
        "x86_64-breenix",
    )
}

fn run_cargo(command: &mut Command, log: &Path, window: Duration) -> CaseResult {
    // Keep the metal driver's 120 s serial-silence watchdog active during
    // a long Cargo run. This progress report does not sample /proc.
    let mut last_progress = Duration::ZERO;
    let build = d1::measure(command, log, window, |_, elapsed| {
        if elapsed - last_progress >= Duration::from_secs(75) {
            case_diagnostic(&format!(
                "SELF_HOST Cargo elapsed_ms={}",
                elapsed.as_millis()
            ));
            last_progress = elapsed;
        }
        Ok(0)
    })
    .map_err(|error| format!("run Cargo: {error}"))?;
    // The shared runner received no RSS samples.
    let _ = build.peak_rss_bytes;
    case_diagnostic(&format!(
        "SELF_HOST Cargo wall_ms={} exit_code={:?} signal={:?} timed_out={}",
        build.wall.as_millis(),
        build.status.code(),
        build.status.signal(),
        build.timed_out,
    ));
    if build.timed_out {
        return Err(format!("Cargo process group exceeded {} ms", window.as_millis()).into());
    }
    if !build.status.success() {
        let reason = build
            .first_error
            .unwrap_or_else(|| format!("Cargo {}", build.status));
        return Err(reason.into());
    }
    Ok(())
}

fn build_package() -> CaseResult {
    let package = running_case()
        .and_then(|name| name.strip_prefix("self-host/build-"))
        .ok_or("missing self-host package")?;
    let index = BUILDS
        .iter()
        .position(|&(name, _)| name == package)
        .ok_or("unknown self-host package")?;
    let run = run_dir()?;
    let before = artifact_snapshot(run)?;
    if index == 0 {
        if fs::read_dir(run.join("target"))
            .map_err(|error| error.to_string())?
            .next()
            .is_some()
        {
            return Err("the first self-host target directory is not empty".into());
        }
    } else {
        verify_record(run, BUILDS[index - 1].0, &before)?;
    }
    let log = run.join(format!("{package}.log"));
    run_cargo(
        &mut cargo(run, package),
        &log,
        Duration::from_millis(BUILDS[index].1),
    )?;
    let after = artifact_snapshot(run)?;
    let library = format!("{PROFILE}/lib{package}.rlib");
    let binary = format!("{PROFILE}/{package}");
    for output in std::iter::once(&library).chain((package != "bobj").then_some(&binary)) {
        if before.contains_key(output) || !after.contains_key(output) {
            return Err(format!("Cargo did not produce a new {output}").into());
        }
    }
    let digests: Digests = after
        .into_iter()
        .map(|(path, artifact)| (path, artifact.sha256))
        .collect();
    let receipt = serde_json::to_vec(&digests).map_err(|error| error.to_string())?;
    let name = format!("{package}.sha256.json");
    let mut record = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(run.join(&name))
        .map_err(|error| format!("create {package} digest record: {error}"))?;
    record
        .write_all(&receipt)
        .map_err(|error| format!("write {package} digest record: {error}"))?;
    let digest: String = sha256::sha256(&receipt)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    case_diagnostic(&format!(
        "SELF_HOST {package} digest_record={}/{name} files={} sha256={digest}",
        run.display(),
        digests.len(),
    ));
    Ok(())
}

fn tools_equal_mac() -> CaseResult {
    let run = run_dir()?;
    let current = artifact_snapshot(run)?;
    verify_record(run, "bcc", &current)?;
    let mac: serde_json::Value = serde_json::from_str(MAC_REFERENCE)
        .map_err(|error| format!("parse Mac twin record: {error}"))?;
    let mut matched = Digests::new();
    for tool in TOOLS {
        let path = format!("{PROFILE}/{tool}");
        let output = current
            .get(&path)
            .ok_or_else(|| format!("missing rebuilt tool {path}"))?;
        let expected = mac["outputs"][tool]["sha256"]
            .as_str()
            .ok_or_else(|| format!("missing Mac SHA-256 for {tool}"))?;
        if output.sha256 != expected {
            return Err(format!("{tool} differs from the Mac twin: {}", output.sha256).into());
        }
        case_diagnostic(&format!("SELF_HOST {tool} mac_sha256={expected}"));
        matched.insert(tool.into(), expected.into());
    }
    let receipt = serde_json::to_vec(&matched).map_err(|error| error.to_string())?;
    fs::write(run.join(MAC_MATCH), receipt)
        .map_err(|error| format!("write Mac comparison record: {error}"))?;
    Ok(())
}

fn rebuilt_tools_behave() -> CaseResult {
    let run = run_dir()?;
    let receipt = fs::read(run.join(MAC_MATCH))
        .map_err(|error| format!("read Mac comparison record: {error}"))?;
    let matched: Digests = serde_json::from_slice(&receipt)
        .map_err(|error| format!("parse Mac comparison record: {error}"))?;
    self_host_behave::exercise(run, &matched).map_err(Into::into)
}

fn check_fresh(log: &str) -> CaseResult {
    let mut fresh = BTreeSet::new();
    for line in log.lines() {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("Compiling" | "Running") => {
                return Err(format!("Cargo rebuilt: {}", line.trim()).into())
            }
            Some("Fresh") => {
                let package = fields
                    .next()
                    .ok_or("Cargo Fresh status omits the package")?;
                fresh.insert(package);
            }
            _ => {}
        }
    }
    for package in d1::PACKAGES {
        if !fresh.contains(package) {
            return Err(format!("Cargo did not report {package} Fresh").into());
        }
    }
    Ok(())
}

fn rebuilt_cargo_noop() -> CaseResult {
    let run = run_dir()?;
    let result = check_cargo_noop(run);
    sysfs::umount2(SOURCE, 0).map_err(|error| format!("detach self-host fixture: {error}"))?;
    result
}

fn check_cargo_noop(run: &Path) -> CaseResult {
    let before = artifact_snapshot(run)?;
    verify_record(run, "bcc", &before)?;
    let mut command = cargo(run, TOOLS[0]);
    for package in &TOOLS[1..] {
        command.args(["-p", package]);
    }
    command.arg("-v");
    let log = run.join("rebuilt-cargo-noop.log");
    run_cargo(&mut command, &log, Duration::from_secs(10))?;
    let cargo_output =
        fs::read_to_string(log).map_err(|error| format!("read Cargo log: {error}"))?;
    check_fresh(&cargo_output)?;
    verify_noop(&before, &artifact_snapshot(run)?).map_err(Into::into)
}

fn main() {
    SUITE.run();
}
