//! btrace — Breenix Trace Reader
//!
//! Reads kernel and xHCI trace data from procfs and prints a compact
//! diagnostic summary to stdout (which goes to serial). Runs once,
//! suitable for being called periodically by init or from the shell.
//!
//! Reads:
//!   /proc/uptime         — system uptime
//!   /proc/stat           — syscalls, interrupts, context switches, etc.
//!   /proc/trace/buffer   — kernel trace ring, summarized for diagnostic events
//!   /proc/xhci/trace     — clean-room xHCI runtime state
//!
//! Output format:
//!   === btrace @ 42.5s ===
//!   sys=1234 irq=5678 ctx=901 fork=12 exec=8 cow=3
//!   xhci: xhci initialized=true interrupts=156
//!   =====================

use libbreenix::fs::{self, O_RDONLY};
use libbreenix::io;

/// Read the full contents of a procfs file into a Vec<u8>.
fn read_procfs(path: &str) -> Vec<u8> {
    match fs::open(path, O_RDONLY) {
        Ok(fd) => {
            let mut buf = vec![0u8; 262144];
            let mut total = 0;
            loop {
                match io::read(fd, &mut buf[total..]) {
                    Ok(n) if n > 0 => {
                        total += n;
                        if total >= buf.len() - 256 {
                            buf.resize(buf.len() + 4096, 0);
                        }
                    }
                    _ => break,
                }
            }
            let _ = io::close(fd);
            buf.truncate(total);
            buf
        }
        Err(_) => Vec::new(),
    }
}

/// Parse a "key value" line from /proc/stat, returning the value as u64.
fn parse_stat_value(stat: &str, key: &str) -> u64 {
    for line in stat.lines() {
        if line.starts_with(key) {
            if let Some(val_str) = line.split_whitespace().nth(1) {
                return val_str.parse().unwrap_or(0);
            }
        }
    }
    0
}

/// Parse uptime seconds from /proc/uptime ("42.50 0.00\n").
fn parse_uptime(data: &[u8]) -> (u64, u64) {
    let s = core::str::from_utf8(data).unwrap_or("");
    let first = s.split_whitespace().next().unwrap_or("0.0");
    if let Some(dot) = first.find('.') {
        let secs: u64 = first[..dot].parse().unwrap_or(0);
        let frac: u64 = first[dot + 1..].parse().unwrap_or(0);
        (secs, frac)
    } else {
        (first.parse().unwrap_or(0), 0)
    }
}

struct TraceDiagSummary<'a> {
    ctx_diag: u64,
    sched_diag: u64,
    first_ctx: Option<&'a str>,
    first_sched: Option<&'a str>,
}

fn parse_trace_diag(data: &[u8]) -> TraceDiagSummary<'_> {
    let text = core::str::from_utf8(data).unwrap_or("");
    let mut summary = TraceDiagSummary {
        ctx_diag: 0,
        sched_diag: 0,
        first_ctx: None,
        first_sched: None,
    };

    for line in text.lines() {
        if line.contains("CTX_DIAG") {
            summary.ctx_diag += 1;
            if summary.first_ctx.is_none() {
                summary.first_ctx = Some(line);
            }
        }
        if line.contains("SCHED_DIAG") {
            summary.sched_diag += 1;
            if summary.first_sched.is_none() {
                summary.first_sched = Some(line);
            }
        }
    }

    summary
}

/// Completion code to human-readable name.
fn main() {
    // Read procfs files
    let uptime_data = read_procfs("/proc/uptime");
    let stat_data = read_procfs("/proc/stat");
    let trace_data = read_procfs("/proc/trace/buffer");
    let xhci_data = read_procfs("/proc/xhci/trace");

    // Parse uptime
    let (up_secs, up_frac) = parse_uptime(&uptime_data);

    // Parse /proc/stat counters
    let stat_str = core::str::from_utf8(&stat_data).unwrap_or("");
    let syscalls = parse_stat_value(stat_str, "syscalls");
    let irqs = parse_stat_value(stat_str, "interrupts");
    let ctx = parse_stat_value(stat_str, "context_switches");
    let forks = parse_stat_value(stat_str, "forks");
    let execs = parse_stat_value(stat_str, "execs");
    let cow = parse_stat_value(stat_str, "cow_faults");

    // Parse xHCI trace
    let trace_diag = parse_trace_diag(&trace_data);

    // Print compact summary
    print!("=== btrace @ {}.{}s ===\n", up_secs, up_frac);
    print!(
        "sys={} irq={} ctx={} fork={} exec={} cow={}\n",
        syscalls, irqs, ctx, forks, execs, cow,
    );
    print!(
        "trace_diag: CTX_DIAG={} SCHED_DIAG={}\n",
        trace_diag.ctx_diag, trace_diag.sched_diag,
    );
    if let Some(line) = trace_diag.first_ctx {
        print!("trace_diag:first_ctx={}\n", line);
    }
    if let Some(line) = trace_diag.first_sched {
        print!("trace_diag:first_sched={}\n", line);
    }

    match core::str::from_utf8(&xhci_data) {
        Ok(text) if !text.is_empty() => {
            for line in text.lines() {
                print!("xhci: {}\n", line);
            }
        }
        _ => print!("xhci: runtime state unavailable\n"),
    }

    print!("=====================\n");
    std::process::exit(0);
}
