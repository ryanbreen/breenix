//! Persistent, bounded kernel logs from non-destructive sequenced proc snapshots.
use libbreenix::service_logs::{Lines, Log};
use std::path::Path;

fn main() {
    let boot = std::fs::read_to_string("/run/breenix/boot-id").expect("blogd boot ID");
    let mut log = Log::new(Path::new("/var/log"), "kernel", boot.trim()).expect("kernel log");
    let mut lines = Lines::new();
    let mut next = 0u64;
    println!("[blogd] logging kernel snapshots boot={}", boot.trim());
    loop {
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
                                log.record(format!("[log] lost lines=unknown bytes={} reason=ring-overrun\n", start - next).as_bytes());
                                next = start;
                            }
                            if end >= next && next >= start {
                                for chunk in data[(next - start) as usize..].chunks(4096) {
                                    lines.feed(chunk, "kernel", &mut log);
                                }
                                next = end;
                            }
                        }
                    }
                }
            }
        }
        let _ = libbreenix::time::sleep_ms(250);
    }
}
