//! Persistent, bounded kernel logs from non-destructive sequenced proc snapshots.
use libbreenix::service_logs::KmsgFollower;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: i32) { STOP.store(true, Ordering::Relaxed); }

fn main() {
    libbreenix::signal::sigaction(libbreenix::signal::SIGTERM,
        Some(&libbreenix::signal::Sigaction::new(stop)), None).expect("blogd stop handler");
    let boot = std::fs::read_to_string("/run/breenix/boot-id").expect("blogd boot ID");
    let mut directory = PathBuf::from("/var/log");
    let mut state = PathBuf::from(format!("/run/breenix/kmsg-{}", boot.trim()));
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let target = if arg == "--dir" { &mut directory }
            else if arg == "--state" { &mut state }
            else { panic!("usage: blogd [--dir DIR] [--state FILE]") };
        *target = args.next().expect("blogd option requires a path").into();
    }
    let mut follower = None;
    let mut failures = 0u64;
    println!("[blogd] logging kernel snapshots boot={}", boot.trim());
    while !STOP.load(Ordering::Relaxed) {
        let result = (|| -> std::io::Result<()> {
            if follower.is_none() {
                follower = Some(KmsgFollower::restore(&directory, &state, boot.trim())?);
            }
            let snapshot = std::fs::read_to_string("/proc/kmsg-sequence")?;
            follower.as_mut().unwrap().apply(&snapshot)
        })();
        if let Err(error) = result {
            // An append may have reached disk without its cursor. Restore the
            // committed files and partial input before the next attempt.
            follower = None;
            failures = failures.saturating_add(1);
            if failures == 1 { eprintln!("[blogd] kernel log unavailable; retrying: {error}"); }
        } else if failures != 0 {
            println!("[blogd] kernel log recovered after failed attempts={failures}");
            failures = 0;
        }
        let _ = libbreenix::time::sleep_ms(250);
    }
}
