//! Ruling 126's opt-in Rust self-host build diagnostic.
#[path = "suite_support/d1.rs"]
mod d1;

use breenix_suite::suite::{
    case, case_diagnostic, category, running_case, suite, value, CaseError, CaseResult, Suite,
};
use libbreenix::{crypto::rand::getrandom_bytes, fs as sysfs, process};
use std::fs;
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

const SOURCE: &str = "/src";
const FIXTURE: &str = "/usr/libexec/breenix/rustc";
const MARKER: &str = "/src/breenix-d1";
static RUN: OnceLock<Result<PathBuf, String>> = OnceLock::new();

static SUITE: Suite = suite(
    "d1",
    "Self-host diagnostic",
    &[category(
        "build-profile",
        "Cargo package measurements",
        &[
            case("bobj", "Measure bobj", measure_package),
            case("bas", "Measure bas", measure_package),
            case("bld", "Measure bld", measure_package),
            case("bar", "Measure bar", measure_package),
            case("bnm", "Measure bnm", measure_package),
            case("bcc", "Measure bcc", measure_package),
            case("bmake", "Measure bmake", measure_last_package),
        ],
    )],
)
// Ruling 126 grants 600 s to Cargo. The suite watchdog leaves 10 s to reap and report.
.case_limit_ms(610_000)
.with_case_observer(prepare_scratch);

fn prepare_scratch(completed: usize, _: usize, _: &str) {
    // The runner takes ownership after the sequence reset and before the first fork.
    if completed == 0 {
        let _ = RUN.set(mount_fixture());
    }
}

fn mount_fixture() -> Result<PathBuf, String> {
    fs::create_dir_all(SOURCE).map_err(|e| format!("create {SOURCE}: {e}"))?;
    let mut fixtures = Vec::new();
    for entry in fs::read_dir("/dev").map_err(|e| format!("list /dev: {e}"))? {
        let path = entry.map_err(|e| format!("read /dev entry: {e}"))?.path();
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
        // SAFETY: these are valid paths and BreenFS takes no mount data.
        if unsafe { sysfs::mount(device, SOURCE, "breenfs", 1, std::ptr::null()) }.is_err() {
            continue;
        }
        let marked = fs::read(MARKER).is_ok_and(|bytes| bytes == b"e25-d1\n");
        sysfs::umount2(SOURCE, 0).map_err(|e| format!("detach {device}: {e}"))?;
        if marked {
            fixtures.push(path);
        }
    }
    let [device] = fixtures.as_slice() else {
        return Err(format!(
            "expected one D1 scratch fixture, found {fixtures:?}"
        ));
    };
    let device = device.to_str().ok_or("invalid D1 device path")?;
    // SAFETY: only the read-only-probed, uniquely marked fixture is mounted for writes.
    unsafe { sysfs::mount(device, SOURCE, "breenfs", 0, std::ptr::null()) }
        .map_err(|e| format!("mount {device} at {SOURCE}: {e}"))?;
    let setup = (|| {
        let mut nonce = [0; 16];
        if getrandom_bytes(&mut nonce) != Ok(nonce.len()) {
            return Err("could not generate the D1 boot nonce".into());
        }
        let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
        let run = Path::new(SOURCE).join(nonce);
        fs::create_dir(&run).map_err(|e| format!("create D1 run directory: {e}"))?;
        for name in ["target", "tmp", "cargo-home/registry"] {
            fs::create_dir_all(run.join(name)).map_err(|e| format!("create D1 {name}: {e}"))?;
        }
        Ok(run)
    })();
    if setup.is_err() {
        sysfs::umount2(SOURCE, 0)
            .map_err(|e| format!("detach D1 fixture after setup failure: {e}"))?;
    }
    setup
}

fn measure_last_package() -> CaseResult {
    let result = measure_package();
    if RUN.get().is_some_and(Result::is_ok) {
        sysfs::umount2(SOURCE, 0).map_err(|error| format!("detach D1 scratch fixture: {error}"))?;
    }
    result
}

fn scratch_bytes() -> io::Result<u64> {
    let report = fs::read_to_string("/proc/breenix/fs-mutation-costs")?;
    report
        .lines()
        .find_map(|line| {
            line.strip_prefix("/src breenfs mutation_costs=1 write_bytes=")?
                .parse()
                .ok()
        })
        .ok_or_else(|| io::Error::other("missing D1 scratch write counter"))
}

fn group_rss(group: i32) -> io::Result<u64> {
    let mut bytes = 0;
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        if !process::getpgid(pid).is_ok_and(|pgid| pgid.raw() == group as u64) {
            continue;
        }
        // A process may exit between directory enumeration and stat.
        if let Ok(stat) = fs::read_to_string(entry.path().join("stat")) {
            if let Some((_, fields)) = stat.rsplit_once(") ") {
                // /proc/<pid>/stat fields 3..24, with RSS in 4 KiB pages.
                if let Some(pages) = fields
                    .split_whitespace()
                    .nth(21)
                    .and_then(|n| n.parse::<u64>().ok())
                {
                    bytes += pages * 4096;
                }
            }
        }
    }
    Ok(bytes)
}

fn measure_package() -> CaseResult {
    let package = running_case()
        .and_then(|name| name.strip_prefix("build-profile/"))
        .filter(|package| d1::PACKAGES.contains(package))
        .ok_or("missing D1 package")?;
    let run = RUN
        .get()
        .ok_or("missing D1 fixture setup")?
        .as_ref()
        .map_err(|error| CaseError::Fail(error.clone()))?;
    let before = scratch_bytes().map_err(|error| error.to_string())?;
    let mut last_progress = Duration::ZERO;
    let log = Path::new("/tmp").join(format!("d1-{}-{package}.log", std::process::id()));
    let result = d1::measure(
        &mut d1::cargo(
            Path::new(SOURCE),
            Path::new(FIXTURE),
            &Path::new(FIXTURE).join("bin/bld"),
            run,
            package,
            "x86_64-breenix",
        ),
        &log,
        Duration::from_secs(600),
        |group, elapsed| {
            if elapsed - last_progress >= Duration::from_secs(75) {
                value("elapsed", elapsed.as_secs() as i64, "s", None);
                last_progress = elapsed;
            }
            group_rss(group)
        },
    );
    let _ = fs::remove_file(log);
    let build = result.map_err(|error| error.to_string())?;
    let written = scratch_bytes()
        .map_err(|error| error.to_string())?
        .checked_sub(before)
        .ok_or("D1 scratch counter regressed")?;
    let wall_ms = build.wall.as_millis() as i64;
    value("wall", wall_ms, "ms", None);
    value("peak-rss", build.peak_rss_bytes as i64, "bytes", None);
    value("scratch-written", written as i64, "bytes", None);
    let mut record = serde_json::json!({
        "package": package,
        "nonce": run.file_name().and_then(|name| name.to_str()),
        "wall_ms": wall_ms,
        "peak_rss_bytes": build.peak_rss_bytes,
        "scratch_bytes_written": written,
        "exit_code": build.status.code(),
        "signal": build.status.signal(),
        "over_60_s": build.wall > Duration::from_secs(60),
        "timed_out": build.timed_out,
        "first_error": build.first_error,
    });
    let mut line = format!("D1 {record}");
    if line.len() > 1024 {
        let short: String = record["first_error"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_control())
            .take(96)
            .collect();
        record["first_error"] = format!("{short}…").into();
        line = format!("D1 {record}");
    }
    case_diagnostic(&line);
    Ok(())
}

fn main() {
    SUITE.run();
}
