//! Build two copies of the workspace with the Mac twin and this tree's host bld.
#[path = "../../userspace/programs/src/suite_support/d1.rs"]
mod d1;

use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::process::Command;
use std::time::Duration;

fn checked(command: &mut Command) -> io::Result<()> {
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{command:?}: {status}")))
    }
}

fn verify_digest(
    repo: &std::path::Path,
    arch: &str,
    component: &str,
    binary: &std::path::Path,
) -> io::Result<()> {
    let (record, key) = if component == "rustc" {
        ("k5", "mac_rustc_sha256")
    } else {
        ("k7", "mac_cargo_sha256")
    };
    let json = fs::read_to_string(repo.join(format!("toolchain/rust-host/{record}/{arch}.json")))?;
    let marker = format!("\"{key}\": \"");
    let expected = json
        .split(&marker)
        .nth(1)
        .and_then(|s| s.split('"').next())
        .ok_or_else(|| io::Error::other("missing digest"))?;
    let output = Command::new("/usr/bin/shasum")
        .arg("-a")
        .arg("256")
        .arg(binary)
        .output()?;
    let digest = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || digest.split_whitespace().next() != Some(expected) {
        return Err(io::Error::other(format!(
            "{component} digest does not match {record}/{arch}.json"
        )));
    }
    Ok(())
}

fn main() -> io::Result<()> {
    assert_eq!(d1::PACKAGES.len(), 7);
    let mut args = std::env::args_os().skip(1);
    let arch = args.next().expect("architecture");
    let arch = arch.to_str().expect("architecture is UTF-8");
    assert!(matches!(arch, "x86_64" | "aarch64"));
    let target = format!("{arch}-breenix");
    let repo = fs::canonicalize(args.next().expect("repository"))?;
    let fixture = fs::canonicalize(args.next().expect("K6 fixture"))?;
    let work = fs::canonicalize(args.next().expect("work directory"))?;
    let rustc = fs::canonicalize(
        std::env::var_os("BREENIX_MAC_RUSTC").expect("set BREENIX_MAC_RUSTC to the K5 twin"),
    )?;
    let cargo = fs::canonicalize(
        std::env::var_os("BREENIX_MAC_CARGO").expect("set BREENIX_MAC_CARGO to the K7 Mac Cargo"),
    )?;
    verify_digest(&repo, arch, "rustc", &rustc)?;
    verify_digest(&repo, arch, "cargo", &cargo)?;
    let host_target = work.join("host-target");
    checked(
        Command::new("cargo")
            .current_dir(&repo)
            .args([
                "build",
                "--offline",
                "--locked",
                "--release",
                "--manifest-path",
            ])
            .arg(repo.join("toolchain/Cargo.toml"))
            .args(["-p", "bld", "--target-dir"])
            .arg(&host_target),
    )?;
    let linker = host_target.join("release/bld");
    for name in ["one", "two"] {
        let source = work.join(name);
        fs::create_dir(&source)?;
        let archive = work.join(format!("{name}.tar"));
        checked(
            Command::new("git")
                .current_dir(&repo)
                .args(["archive", "--format=tar", "-o"])
                .arg(&archive)
                .args(["HEAD", "toolchain"]),
        )?;
        checked(
            Command::new("tar")
                .args(["-xf"])
                .arg(&archive)
                .arg("-C")
                .arg(&source),
        )?;
        let staged = source.join("fixture");
        fs::create_dir(&staged)?;
        fs::create_dir(staged.join("bin"))?;
        symlink(&rustc, staged.join("bin/rustc"))?;
        symlink(&cargo, staged.join("bin/cargo"))?;
        for (from, to) in [
            ("sysroot", "sysroot"),
            ("targets", "targets"),
            ("lib/tools", "lib"),
        ] {
            symlink(fixture.join(from), staged.join(to))?;
        }
        let run = source.join("run");
        fs::create_dir(&run)?;
        fs::create_dir(run.join("tmp"))?;
        fs::create_dir_all(run.join("cargo-home/registry"))?;
        for package in ["bobj", "bas", "bld", "bar", "bnm", "bmake", "bcc"] {
            let cargo_command = d1::cargo(&source, &staged, &linker, &run, package, &target);
            let mut timed = Command::new("/usr/bin/time");
            timed
                .arg("-l")
                .arg(cargo_command.get_program())
                .args(cargo_command.get_args());
            timed.env_clear().envs(
                cargo_command
                    .get_envs()
                    .filter_map(|(key, value)| value.map(|value| (key, value))),
            );
            timed.current_dir(
                cargo_command
                    .get_current_dir()
                    .expect("cargo working directory"),
            );
            let result = d1::measure(
                &mut timed,
                &source.join(format!("{package}.log")),
                Duration::from_secs(600),
                |_, _| Ok(0),
            )?;
            println!(
                "{name} {package}: {} ms, {}",
                result.wall.as_millis(),
                result.status
            );
            if result.timed_out || !result.status.success() {
                return Err(io::Error::other(format!(
                    "{package}: {:?}",
                    result.first_error
                )));
            }
            let log = fs::read_to_string(source.join(format!("{package}.log")))?;
            let rss = log
                .lines()
                .find_map(|line| {
                    line.trim()
                        .strip_suffix("  maximum resident set size")
                        .and_then(|n| n.trim().parse::<u64>().ok())
                })
                .ok_or_else(|| io::Error::other("missing time peak RSS"))?;
            println!("{name} {package}: peak RSS {rss} bytes");
            let _ = result.peak_rss_bytes;
        }
    }
    Ok(())
}
