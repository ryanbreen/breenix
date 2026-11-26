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

fn main() -> io::Result<()> {
    assert_eq!(d1::PACKAGES.len(), 7);
    let mut args = std::env::args_os().skip(1);
    let repo = fs::canonicalize(args.next().expect("repository"))?;
    let fixture = fs::canonicalize(args.next().expect("K6 fixture"))?;
    let work = fs::canonicalize(args.next().expect("work directory"))?;
    let rustc = fs::canonicalize(
        std::env::var_os("BREENIX_MAC_RUSTC").expect("set BREENIX_MAC_RUSTC to the K5 twin"),
    )?;
    let cargo = fs::canonicalize(
        std::env::var_os("BREENIX_MAC_CARGO").expect("set BREENIX_MAC_CARGO to the K7 Mac Cargo"),
    )?;
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
            let result = d1::measure(
                &mut d1::cargo(&source, &staged, &linker, &run, package),
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
            let _ = result.peak_rss_bytes;
        }
    }
    Ok(())
}
