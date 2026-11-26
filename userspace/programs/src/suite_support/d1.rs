//! Shared Cargo invocation and D1 process bookkeeping.
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

pub const PACKAGES: [&str; 7] = ["bobj", "bas", "bld", "bar", "bnm", "bcc", "bmake"];

pub fn cargo(source: &Path, fixture: &Path, linker: &Path, run: &Path, package: &str) -> Command {
    let flags = [
        format!("--sysroot={}", fixture.join("sysroot").display()),
        format!("-Clinker={}", linker.display()),
        format!("-Clink-arg=-T{}", fixture.join("lib/linker.ld").display()),
        format!("-Lnative={}", fixture.join("lib").display()),
        "-Clink-arg=--allow-multiple-definition".into(),
        "-Cdefault-linker-libraries=no".into(),
        format!("--remap-path-prefix={}=/breenix-src", source.display()),
    ];
    let mut command = Command::new(fixture.join("bin/cargo"));
    command
        .env_clear()
        .env("PATH", "/bin")
        .env("HOME", run.join("cargo-home"))
        .env("CARGO_HOME", run.join("cargo-home"))
        .env("CARGO_BUILD_JOBS", "4")
        .env("CARGO_TERM_COLOR", "never")
        .env("RUSTC", fixture.join("bin/rustc"))
        .env("RUST_TARGET_PATH", fixture.join("targets"))
        .env("CARGO_ENCODED_RUSTFLAGS", flags.join("\u{1f}"))
        .env("TMPDIR", run.join("tmp"))
        .current_dir(source.join("toolchain"))
        .args([
            "build",
            "--offline",
            "--frozen",
            "--profile",
            "selfhost",
            "--target",
        ])
        .arg(fixture.join("targets/x86_64-breenix.json"))
        .arg("--manifest-path")
        .arg(source.join("toolchain/Cargo.toml"))
        .args(["-p", package, "--target-dir"])
        .arg(run.join("target"));
    command
}

pub struct Build {
    pub wall: Duration,
    pub peak_rss_bytes: u64,
    pub status: ExitStatus,
    pub timed_out: bool,
    pub first_error: Option<String>,
}

pub fn measure(
    command: &mut Command,
    log: &Path,
    allowance: Duration,
    mut sample_rss: impl FnMut(i32, Duration) -> io::Result<u64>,
) -> io::Result<Build> {
    let output = File::create(log)?;
    command
        .process_group(0)
        .stdout(output.try_clone()?)
        .stderr(output);
    let start = Instant::now();
    let mut child = command.spawn()?;
    let group = i32::try_from(child.id()).expect("PID fits pid_t");
    let mut peak_rss_bytes = 0;
    let observation: io::Result<Option<ExitStatus>> = (|| loop {
        let elapsed = start.elapsed();
        if elapsed >= allowance {
            return Ok(None);
        }
        peak_rss_bytes = peak_rss_bytes.max(sample_rss(group, elapsed)?);
        if start.elapsed() >= allowance {
            return Ok(None);
        }
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        std::thread::sleep(
            Duration::from_millis(50).min(allowance.saturating_sub(start.elapsed())),
        );
    })();
    let wall = start.elapsed();
    let (status, timed_out) = match observation {
        Ok(Some(status)) => (status, false),
        result => {
            let killed = kill_group(group);
            let status = child.wait();
            result?;
            killed?;
            (status?, true)
        }
    };
    let first_error = if status.success() && !timed_out {
        None
    } else {
        first_diagnostic(log)?.or_else(|| {
            Some(if timed_out {
                "Cargo process group exceeded its deadline".into()
            } else {
                format!("Cargo {status}")
            })
        })
    };
    Ok(Build {
        wall,
        peak_rss_bytes,
        status,
        timed_out,
        first_error,
    })
}

fn first_diagnostic(log: &Path) -> io::Result<Option<String>> {
    let mut reader = BufReader::new(fs::File::open(log)?);
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line)? != 0 {
        let text = String::from_utf8_lossy(&line);
        let text = text.trim();
        if text.starts_with("error:")
            || text.starts_with("error[")
            || text.starts_with("fatal error:")
        {
            return Ok(Some(text.to_owned()));
        }
        line.clear();
    }
    Ok(None)
}

#[cfg(target_os = "breenix")]
fn kill_group(group: i32) -> io::Result<()> {
    use libbreenix::{errno::Errno, error::Error, signal};
    match signal::kill(-group, signal::SIGKILL) {
        Ok(()) | Err(Error::Os(Errno::ESRCH)) => Ok(()),
        Err(Error::Os(errno)) => Err(io::Error::from_raw_os_error(errno as i32)),
    }
}

#[cfg(not(target_os = "breenix"))]
fn kill_group(group: i32) -> io::Result<()> {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // SAFETY: the negative PID identifies the group created by Command above.
    if unsafe { kill(-group, 9) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(3) {
        Ok(())
    } else {
        Err(error)
    }
}
