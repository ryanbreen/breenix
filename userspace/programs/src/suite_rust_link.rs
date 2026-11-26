//! Replay the recorded Rust linker invocations with the box linker.
use breenix_suite::suite::{case, category, suite, CaseError, CaseResult, Suite};
use libbreenix::crypto::sha256::Sha256;
use libbreenix::signal::SIGABRT;
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::os::unix::{fs::DirBuilderExt, process::ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

static SUITE: Suite = suite(
    "rust-link",
    "Rust linker",
    &[category(
        "rust-link",
        "Rust linker",
        &[
            case("links-tool-target", "Links the tool target", links_tool_target),
            case("links-hosted-unwind", "Links hosted unwinding", links_hosted_unwind),
            case("rlib-members-skipped", "Skips only rlib metadata", rlib_members_skipped),
        ],
    )],
)
.case_limit_ms(60_000);

const FIXTURE: &str = "/usr/libexec/breenix/rust-link";
const BLD: &str = "/usr/libexec/breenix/rust-link/bin/bld";

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, CaseError> {
        let path = PathBuf::from(format!("/tmp/rust-link-{}", std::process::id()));
        io_at(&path, fs::DirBuilder::new().mode(0o700).create(&path))?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn io_at<T>(path: &Path, result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|error| format!("{}: {error}", path.display()))
}

fn read_json(name: &str) -> Result<Value, CaseError> {
    let path = Path::new(FIXTURE).join(name);
    let bytes = io_at(&path, fs::read(&path))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{name}: {error}").into())
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, CaseError> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("missing or invalid {key}").into())
}

fn verify_digest(path: &Path, expected: &str) -> CaseResult {
    let mut file = io_at(path, fs::File::open(path))?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 16384];
    loop {
        let count = io_at(path, file.read(&mut buffer))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let actual: String = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != expected {
        return Err(format!("{}: SHA-256 {actual}, expected {expected}", path.display()).into());
    }
    Ok(())
}

fn link(
    name: &str,
    destination: &Path,
    replacement: Option<(&str, &str)>,
    extra_args: &[&str],
) -> Result<Output, CaseError> {
    let receipt = read_json("x86_64.json")?;
    let line = receipt["links"][name]["argv"]
        .as_array()
        .ok_or_else(|| format!("{name}: missing argv"))?;
    let mut args = line
        .iter()
        .map(|arg| {
            arg.as_str()
                .ok_or_else(|| format!("{name}: invalid argv entry"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let output = args
        .iter()
        .position(|arg| *arg == "-o")
        .and_then(|index| args.get_mut(index + 1))
        .ok_or_else(|| format!("{name}: missing -o operand"))?;
    *output = destination.to_str().ok_or("invalid scratch path")?;
    if let Some((original, control)) = replacement {
        let input = args
            .iter_mut()
            .find(|arg| **arg == original)
            .ok_or_else(|| format!("{name}: missing archive {original}"))?;
        *input = control;
    }
    let bld = Path::new(BLD);
    Ok(io_at(
        bld,
        Command::new(bld)
            .current_dir(FIXTURE)
            .env_clear()
            .args(args)
            .args(extra_args)
            .output(),
    )?)
}

fn link_verified(
    name: &str,
    destination: &Path,
    expected: &Value,
    extra_args: &[&str],
) -> CaseResult {
    let linked = link(name, destination, None, extra_args)?;
    if !linked.status.success() {
        return Err(format!(
            "{name}: bld {}: {}",
            linked.status,
            String::from_utf8_lossy(&linked.stderr)
        )
        .into());
    }
    verify_digest(destination, field(expected, "bld_sha256")?)
}

fn probe(name: &str) -> Result<(Output, Output), CaseError> {
    let expect = read_json("expect.json")?;
    let scratch = Scratch::new()?;
    let candidate = scratch.0.join(name);
    link_verified(name, &candidate, &expect[name], &[])?;
    let reference = Path::new(FIXTURE)
        .join("links")
        .join(name)
        .join("reference.elf");
    verify_digest(&reference, field(&expect[name], "reference_sha256")?)?;
    let run =
        |path: &Path| io_at(path, Command::new(path).current_dir(FIXTURE).env_clear().output());
    Ok((run(&candidate)?, run(&reference)?))
}

fn links_tool_target() -> CaseResult {
    let (candidate, reference) = probe("panic-abort")?;
    if candidate.status.signal() != Some(SIGABRT)
        || candidate.status.into_raw() != reference.status.into_raw()
        || candidate.stdout != reference.stdout
        || candidate.stderr != reference.stderr
    {
        return Err(
            format!("panic-abort mismatch: bld {candidate:?}, rust-lld {reference:?}").into()
        );
    }
    Ok(())
}

fn links_hosted_unwind() -> CaseResult {
    let (candidate, reference) = probe("catch-unwind")?;
    if !candidate.status.success()
        || !reference.status.success()
        || candidate.stdout != b"caught K6 panic\n"
        || candidate.stdout != reference.stdout
        || candidate.stderr != reference.stderr
    {
        return Err(
            format!("catch-unwind mismatch: bld {candidate:?}, rust-lld {reference:?}").into()
        );
    }
    Ok(())
}

fn rlib_members_skipped() -> CaseResult {
    let scratch = Scratch::new()?;
    let expect = read_json("expect.json")?;
    let metadata = &expect["metadata"];
    let archive = field(metadata, "archive")?;
    verify_digest(
        &Path::new(FIXTURE).join(archive),
        field(metadata, "sha256")?,
    )?;
    // Whole-archive selection reaches the extraction-time skip without an index entry.
    link_verified(
        "panic-abort",
        &scratch.0.join("original"),
        &expect["panic-abort"],
        &["--whole-archive", archive, "--no-whole-archive"],
    )?;

    let control = &expect["control"];
    let archive = field(control, "archive")?;
    verify_digest(&Path::new(FIXTURE).join(archive), field(control, "sha256")?)?;
    let original = field(control, "original")?;
    let rejected = link(
        "panic-abort",
        &scratch.0.join("control"),
        Some((original, archive)),
        &[],
    )?;
    let diagnostic = field(control, "diagnostic")?;
    if !matches!(rejected.status.code(), Some(code) if code != 0)
        || rejected.stderr != diagnostic.as_bytes()
    {
        return Err(format!(
            "control link {}: expected {diagnostic:?}, got {:?}",
            rejected.status,
            String::from_utf8_lossy(&rejected.stderr)
        )
        .into());
    }
    Ok(())
}

fn main() {
    SUITE.run()
}
