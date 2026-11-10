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
    let mut follower = KmsgFollower::restore(&directory, &state, boot.trim()).expect("kernel log recovery");
    println!("[blogd] logging kernel snapshots boot={}", boot.trim());
    while !STOP.load(Ordering::Relaxed) {
        if let Ok(snapshot) = std::fs::read_to_string("/proc/kmsg-sequence") {
            follower.apply(&snapshot).expect("kernel log snapshot commit");
        }
        let _ = libbreenix::time::sleep_ms(250);
    }
}
