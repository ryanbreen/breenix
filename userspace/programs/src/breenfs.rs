//! Filesystem administration through a file or directory on the target mount.
#[path = "../../../kernel/src/fs/breenfs/abi.rs"]
mod abi;
use abi::*;
use libbreenix::{error::Error, syscall::{nr, raw}};
use std::{fs::File, os::fd::AsRawFd, time::Duration};

fn ioctl(file: &File, request: u64, arg: u64) -> Result<(), String> {
    let result = unsafe { raw::syscall3(nr::IOCTL, file.as_raw_fd() as u64, request, arg) };
    Error::from_syscall(result as i64).map(|_| ()).map_err(|e| e.to_string())
}
fn status(file: &File) -> Result<ScrubStatus, String> {
    let mut status = ScrubStatus::default();
    ioctl(file, SCRUB_STATUS, &mut status as *mut _ as u64)?;
    if status.bad_count as usize > status.bad.len() { return Err("invalid scrub status".into()); }
    Ok(status)
}
fn show(status: &ScrubStatus) -> Result<(), String> {
    let state = match status.state {
        SCRUB_IDLE => "idle", SCRUB_RUNNING => "running", SCRUB_DONE => "done",
        SCRUB_CANCELLED => "cancelled", SCRUB_FAILED => "failed",
        _ => return Err("unknown scrub state".into()),
    };
    println!("state={state} bytes_scanned={} errors={} last_scrub_generation={} last_scrub_errors={}",
        status.bytes_scanned, status.errors, status.last_scrub_generation, status.last_scrub_errors);
    for bad in &status.bad[..status.bad_count as usize] {
        let kind = match bad.kind { BAD_NODE => "node", BAD_DATA => "data",
            _ => return Err("unknown bad block kind".into()) };
        println!("bad_block={} kind={kind}", bad.block);
    }
    Ok(())
}
fn snapshot(args: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: breenfs snapshot create|clone|delete NAME PATH; breenfs snapshot status|wait PATH";
    let (action, path) = match args {
        [action, name, path] if matches!(action.as_str(), "create" | "clone" | "delete") => {
            let bytes = name.as_bytes();
            if bytes.is_empty() || bytes.len() > 255 || bytes.iter().any(|b| matches!(b, 0 | b'/' | b',')) {
                return Err("invalid snapshot name".into());
            }
            let file = File::open(path).map_err(|e| format!("{path}: {e}"))?;
            let mut request = SnapshotRequest { flags: if action == "create" { SNAPSHOT_READ_ONLY } else { 0 }, name_len: bytes.len() as u32, name: [0; 256] };
            request.name[..bytes.len()].copy_from_slice(bytes);
            let op = if action == "delete" { SNAPSHOT_DELETE } else { SNAPSHOT_CREATE };
            ioctl(&file, op, &mut request as *mut _ as u64)?;
            return Ok(());
        }
        [action, path] if matches!(action.as_str(), "status" | "wait") => (action, path),
        _ => return Err(USAGE.into()),
    };
    let file = File::open(path).map_err(|e| format!("{path}: {e}"))?;
    loop {
        let mut status = SnapshotStatus::default();
        ioctl(&file, SNAPSHOT_STATUS, &mut status as *mut _ as u64)?;
        if action != "wait" || status.dead_remaining == 0 {
            println!("root_id={} flags={} roots={} dead_remaining={}", status.root_id, status.flags, status.roots, status.dead_remaining);
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("snapshot") { return snapshot(&args[1..]); }
    if args.len() != 3 || args[0] != "scrub"
        || !matches!(args[1].as_str(), "start" | "cancel" | "status" | "wait") {
        return Err("usage: breenfs scrub start|cancel|status|wait PATH".into());
    }
    let file = File::open(&args[2]).map_err(|e| format!("{}: {e}", args[2]))?;
    match args[1].as_str() {
        "start" => ioctl(&file, SCRUB_START, 0)?,
        "cancel" => ioctl(&file, SCRUB_CANCEL, 0)?,
        _ => {},
    }
    let mut current = status(&file)?;
    if args[1] == "wait" {
        while current.state == SCRUB_RUNNING {
            std::thread::sleep(Duration::from_millis(100));
            current = status(&file)?;
        }
    }
    show(&current)?;
    if args[1] == "wait" && (current.state != SCRUB_DONE || current.errors != 0) {
        return Err("scrub did not finish cleanly".into());
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("breenfs: {error}");
        std::process::exit(1);
    }
}
