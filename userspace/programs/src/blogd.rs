//! Persistent, bounded kernel logs from non-destructive sequenced proc snapshots.
use libbreenix::service_logs::{Lines, Log};
use std::path::Path;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicBool, Ordering};

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: i32) { STOP.store(true, Ordering::Relaxed); }

fn main() {
    libbreenix::signal::sigaction(libbreenix::signal::SIGTERM,
        Some(&libbreenix::signal::Sigaction::new(stop)), None).expect("blogd stop handler");
    let boot = std::fs::read_to_string("/run/breenix/boot-id").expect("blogd boot ID");
    let mut log = Log::new(Path::new("/var/log"), "kernel", boot.trim()).expect("kernel log");
    let state = format!("/run/breenix/kmsg-{}", boot.trim());
    let (mut next, mut lines) = std::fs::read(&state).ok().and_then(|bytes| {
        let split = bytes.iter().position(|&b| b == b'\n')?;
        let offset = std::str::from_utf8(&bytes[..split]).ok()?.parse().ok()?;
        Some((offset, Lines::restore(&bytes[split + 1..])?))
    }).unwrap_or((0, Lines::new()));
    println!("[blogd] logging kernel snapshots boot={}", boot.trim());
    while !STOP.load(Ordering::Relaxed) {
        if let Ok(snapshot) = std::fs::read_to_string("/proc/kmsg-sequence") {
            if let Some((header, hex)) = snapshot.split_once('\n') {
                let fields: Vec<&str> = header.split_whitespace().collect();
                if fields.len() == 4 && fields[0..2] == ["BREENIX-KMSG", "1"] {
                    if let (Some(start), Some(end)) = (fields[2].strip_prefix("start=").and_then(|s| s.parse::<u64>().ok()),
                        fields[3].strip_prefix("end=").and_then(|s| s.parse::<u64>().ok())) {
                        let data: Option<Vec<u8>> = hex.trim().as_bytes().chunks(2).map(|pair| {
                            std::str::from_utf8(pair).ok().and_then(|s| u8::from_str_radix(s, 16).ok())
                        }).collect();
                        if let Some(data) = data.filter(|data| end >= start && data.len() as u64 == end - start) {
                            if next < start {
                                lines.finish("kernel", &mut log);
                                log.record(format!("[log] lost lines=unknown bytes={} reason=ring-overrun from={} to={}\n", start - next, next, start).as_bytes());
                                next = start;
                            }
                            if end > next && next >= start {
                                for chunk in data[(next - start) as usize..].chunks(4096) {
                                    lines.feed(chunk, "kernel", &mut log);
                                }
                                next = end;
                                let mut saved = format!("{}\n", next).into_bytes();
                                saved.extend_from_slice(&lines.checkpoint());
                                // Atomic replacement keeps the cursor and partial line together.
                                let temporary = format!("{}.new", state);
                                let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true)
                                    .mode(0o600).open(&temporary).expect("kernel log cursor open");
                                file.write_all(&saved).expect("kernel log cursor write");
                                file.sync_all().expect("kernel log cursor sync");
                                std::fs::rename(&temporary, &state).expect("kernel log cursor publish");
                            }
                        }
                    }
                }
            }
        }
        let _ = libbreenix::time::sleep_ms(250);
    }
}
