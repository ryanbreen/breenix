//! Mac entry point for the same command and process bookkeeping as the guest.
#[path = "../../userspace/programs/src/suite_support/d1.rs"]
mod d1;

use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn group_rss(group: i32) -> io::Result<u64> {
    let output = Command::new("/bin/ps")
        .args(["-axo", "pgid=,rss="])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!("ps: {}", output.status)));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            (fields.next()?.parse::<i32>().ok()? == group)
                .then(|| fields.next()?.parse::<u64>().ok())
                .flatten()
        })
        .sum::<u64>()
        * 1024)
}

fn size(path: &Path) -> io::Result<u64> {
    let mut bytes = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        bytes += if metadata.is_dir() {
            size(&entry.path())?
        } else {
            metadata.len()
        };
    }
    Ok(bytes)
}

fn main() -> io::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let source = fs::canonicalize(args.next().expect("usage: d1-mac SOURCE WORK"))?;
    let work = fs::canonicalize(args.next().expect("usage: d1-mac SOURCE WORK"))?;
    let run = work.join("run");
    fs::create_dir(&run)?;
    fs::create_dir(run.join("tmp"))?;
    fs::create_dir(run.join("target"))?;
    let home = source.join("fixture/cargo-home");
    fs::create_dir(&home)?;
    fs::create_dir(home.join("registry"))?;
    let mut previous = size(&run)? + size(&home)?;
    for package in d1::PACKAGES {
        let log = work.join(format!("{package}.log"));
        let build = d1::measure(
            &mut d1::cargo(&source, &run, package),
            &log,
            Duration::from_secs(600),
            |group, _| group_rss(group),
        )?;
        let total = size(&run)? + size(&home)?;
        println!(
            "{package}\t{}\t{}\t{}\t{}\t{}\t{}",
            build.wall.as_millis(),
            build.peak_rss_bytes,
            total.saturating_sub(previous),
            total,
            build.status.code().unwrap_or(-1),
            build.timed_out
        );
        if let Some(error) = build.first_error {
            eprintln!("{package}: {error}");
        }
        previous = total;
    }
    Ok(())
}
