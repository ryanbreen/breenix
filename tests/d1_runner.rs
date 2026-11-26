//! Focused host check: rustc --test tests/d1_runner.rs -o /tmp/d1-check.
#[path = "../userspace/programs/src/suite_support/d1.rs"]
mod d1;

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[test]
fn cargo_runs_one_package_in_a_clean_environment() {
    assert_eq!(
        d1::PACKAGES,
        ["bobj", "bas", "bld", "bar", "bnm", "bcc", "bmake"]
    );
    let fixture = Path::new("/usr/libexec/breenix/rustc");
    let command = d1::cargo(
        Path::new("/src"),
        fixture,
        &fixture.join("bin/bld"),
        Path::new("/src/run"),
        "bld",
    );
    assert_eq!(command.get_program(), "/usr/libexec/breenix/rustc/bin/cargo");
    assert!(command.get_args().eq([
        "build",
        "--offline",
        "--frozen",
        "--profile",
        "selfhost",
        "--target",
        "/usr/libexec/breenix/rustc/targets/x86_64-breenix.json",
        "--manifest-path",
        "/src/toolchain/Cargo.toml",
        "-p",
        "bld",
        "--target-dir",
        "/src/run/target",
    ]));
    let flags = command
        .get_envs()
        .find(|(key, _)| *key == "CARGO_ENCODED_RUSTFLAGS")
        .unwrap()
        .1
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(
        flags.split('\u{1f}').collect::<Vec<_>>(),
        [
            "--sysroot=/usr/libexec/breenix/rustc/sysroot",
            "-Clinker=/usr/libexec/breenix/rustc/bin/bld",
            "-Clink-arg=-T/usr/libexec/breenix/rustc/lib/linker.ld",
            "-Lnative=/usr/libexec/breenix/rustc/lib",
            "-Clink-arg=--allow-multiple-definition",
            "-Cdefault-linker-libraries=no",
            "--remap-path-prefix=/src=/breenix-src",
        ]
    );
}

#[test]
fn records_first_error_peak_and_timeout_and_kills_descendants() {
    let dir = std::env::temp_dir().join(format!("breenix-d1-check-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let log = dir.join("build.log");
    let mut fail = Command::new("/bin/sh");
    fail.args([
        "-c",
        "printf 'Compiling\nerror: first\nerror: second\n' >&2; exit 7",
    ]);
    let failed = d1::measure(&mut fail, &log, Duration::from_secs(2), |_, _| Ok(123)).unwrap();
    assert_eq!(failed.status.code(), Some(7));
    assert_eq!(failed.peak_rss_bytes, 123);
    assert_eq!(failed.first_error.as_deref(), Some("error: first"));
    assert!(!failed.timed_out);

    let marker = dir.join("leaked");
    let mut hang = Command::new("/bin/sh");
    hang.args(["-c", "(sleep 1; touch \"$1\") & wait", "d1-test"])
        .arg(&marker);
    let timed = d1::measure(&mut hang, &log, Duration::from_millis(150), |_, _| Ok(0)).unwrap();
    assert!(timed.timed_out);
    assert!(timed.wall >= Duration::from_millis(150));
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        !marker.exists(),
        "the timed-out child left a running descendant"
    );
    fs::remove_dir_all(dir).unwrap();
}
