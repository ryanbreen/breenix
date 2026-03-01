//! btrace — Breenix Trace Reader
//!
//! Reads kernel and xHCI trace data from procfs and prints a compact
//! diagnostic summary to stdout (which goes to serial). Runs once,
//! suitable for being called periodically by init or from the shell.
//!
//! Reads:
//!   /proc/uptime         — system uptime
//!   /proc/stat           — syscalls, interrupts, context switches, etc.
//!   /proc/xhci/trace     — xHCI hardware trace buffer
//!
//! Output format:
//!   === btrace @ 42.5s ===
//!   sys=1234 irq=5678 ctx=901 fork=12 exec=8 cow=3
//!   xhci: 156 records, last_cc=12 (EP_NOT_ENABLED) slot=1 ep=3
//!   xhci: 45 CMD_COMPLETE 89 XFER_EVENT 22 NOTE
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

/// Completion code to human-readable name.
fn cc_name(cc: u64) -> &'static str {
    match cc {
        0 => "INVALID",
        1 => "SUCCESS",
        2 => "DATA_BUF_ERR",
        3 => "BABBLE",
        4 => "USB_XACT_ERR",
        5 => "TRB_ERR",
        6 => "STALL",
        7 => "RESOURCE_ERR",
        8 => "BANDWIDTH_ERR",
        9 => "NO_SLOTS",
        10 => "INVALID_STREAM_TYPE",
        11 => "SLOT_NOT_ENABLED",
        12 => "EP_NOT_ENABLED",
        13 => "SHORT_PKT",
        14 => "RING_UNDERRUN",
        15 => "RING_OVERRUN",
        16 => "VF_EVENT_RING_FULL",
        17 => "PARAMETER_ERR",
        24 => "CMD_RING_STOPPED",
        25 => "CMD_ABORTED",
        26 => "STOPPED",
        _ => "UNKNOWN",
    }
}

/// Parse the xHCI trace text and extract summary statistics.
struct XhciSummary {
    total_records: u64,
    cmd_complete: u64,
    xfer_event: u64,
    note: u64,
    cmd_submit: u64,
    xfer_submit: u64,
    other: u64,
    last_xfer_cc: u64,
    last_xfer_slot: u64,
    last_xfer_ep: u64,
    // Diagnostic counters from XHCI_DIAG section
    poll_count: u64,
    event_count: u64,
    consumed_xfer_enum: u64,
    first_xfer_cc: u64,
    first_queue_src: u64,
    endpoint_resets: u64,
}

fn parse_xhci_trace(data: &[u8]) -> XhciSummary {
    let mut summary = XhciSummary {
        total_records: 0,
        cmd_complete: 0,
        xfer_event: 0,
        note: 0,
        cmd_submit: 0,
        xfer_submit: 0,
        other: 0,
        last_xfer_cc: 0,
        last_xfer_slot: 0,
        last_xfer_ep: 0,
        poll_count: 0,
        event_count: 0,
        consumed_xfer_enum: 0,
        first_xfer_cc: 0,
        first_queue_src: 0,
        endpoint_resets: 0,
    };

    let text = core::str::from_utf8(data).unwrap_or("");

    // Extract total from header: "=== XHCI_TRACE_START total=N ==="
    for line in text.lines() {
        if line.starts_with("=== XHCI_TRACE_START total=") {
            if let Some(rest) = line.strip_prefix("=== XHCI_TRACE_START total=") {
                if let Some(num_str) = rest.split_whitespace().next() {
                    summary.total_records = num_str.parse().unwrap_or(0);
                }
            }
            continue;
        }

        // Parse trace record lines: "T NNNN OP_NAME    S=SS E=EE TS=... LEN=..."
        if !line.starts_with("T ") {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 6 {
            continue;
        }

        let op = parts[2]; // OP_NAME
        match op {
            "CMD_COMPLETE" => summary.cmd_complete += 1,
            "XFER_EVENT" => summary.xfer_event += 1,
            "NOTE" => summary.note += 1,
            "CMD_SUBMIT" => summary.cmd_submit += 1,
            "XFER_SUBMIT" => summary.xfer_submit += 1,
            _ => summary.other += 1,
        }

        // For XFER_EVENT and CMD_COMPLETE, extract slot and endpoint from S=SS E=EE
        if op == "XFER_EVENT" || op == "CMD_COMPLETE" {
            // parts[3] is "S=NN", parts[4] is "E=NN"
            if let Some(s_val) = parts[3].strip_prefix("S=") {
                summary.last_xfer_slot = s_val.parse().unwrap_or(0);
            }
            if let Some(e_val) = parts[4].strip_prefix("E=") {
                summary.last_xfer_ep = e_val.parse().unwrap_or(0);
            }

            // For XFER_EVENT, the completion code is in the TRB payload.
            // The TRB data is on the NEXT line as hex. The CC is in bits 31:24
            // of the 3rd dword (bytes 8-11). We can look at subsequent hex line.
            // However, parsing raw hex from the dump is complex. For a simpler approach,
            // we check the payload bytes if available on the next line.
            // For now, we'll leave last_xfer_cc=0 and populate it from hex if we can.
        }
    }

    // Parse XHCI_DIAG section for diagnostic counters.
    let mut in_diag = false;
    for line in text.lines() {
        if line.starts_with("=== XHCI_DIAG ===") {
            in_diag = true;
            continue;
        }
        if line.starts_with("=== XHCI_DIAG_END ===") {
            break;
        }
        if in_diag {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let val: u64 = parts[1].parse().unwrap_or(0);
                match parts[0] {
                    "poll_count" => summary.poll_count = val,
                    "event_count" => summary.event_count = val,
                    "consumed_xfer_enum" => summary.consumed_xfer_enum = val,
                    "first_xfer_cc" => summary.first_xfer_cc = val,
                    "first_queue_src" => summary.first_queue_src = val,
                    "endpoint_resets" => summary.endpoint_resets = val,
                    _ => {}
                }
            }
        }
    }

    // Second pass: extract completion codes from XFER_EVENT payload lines.
    // Each XFER_EVENT record is followed by 1-2 hex dump lines (16 bytes = TRB).
    // Completion code is in the "completion" dword: byte 8..12, bits 31:24.
    // Format: "  XXXXXXXX XXXXXXXX XXXXXXXX XXXXXXXX"
    let mut in_xfer_event = false;
    for line in text.lines() {
        if line.starts_with("T ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            in_xfer_event = parts.len() >= 3 && parts[2] == "XFER_EVENT";
            continue;
        }

        if in_xfer_event && line.starts_with("  ") {
            // Parse the hex line. We need the 3rd dword (bytes 8-11).
            // Format: "  AABBCCDD EEFFGGHH IIJJKKLL MMNNOOPP"
            let hex_parts: Vec<&str> = line.trim().split_whitespace().collect();
            if hex_parts.len() >= 3 {
                // 3rd dword is hex_parts[2], CC is top byte (bits 31:24)
                if let Some(dword_str) = hex_parts.get(2) {
                    if dword_str.len() >= 2 {
                        // First two hex chars are the CC (big-endian display of LE bytes)
                        // Actually the hex dump shows raw bytes in order. For a TRB:
                        //   dword[2] at offset 8 displayed as bytes 08 09 0A 0B
                        // The completion code is the highest byte of the dword,
                        // which in little-endian is byte 11 (the 4th byte of dword 2).
                        // In the hex dump: bytes are in memory order.
                        // So for "IIJJKKLL", CC = LL (last byte pair).
                        let len = dword_str.len();
                        if len >= 2 {
                            let cc_hex = &dword_str[len - 2..];
                            if let Ok(cc) = u64::from_str_radix(cc_hex, 16) {
                                summary.last_xfer_cc = cc;
                            }
                        }
                    }
                }
            }
            in_xfer_event = false;
        }
    }

    summary
}

fn main() {
    // Read procfs files
    let uptime_data = read_procfs("/proc/uptime");
    let stat_data = read_procfs("/proc/stat");
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
    let xhci = parse_xhci_trace(&xhci_data);

    // Print compact summary
    print!("=== btrace @ {}.{}s ===\n", up_secs, up_frac);
    print!(
        "sys={} irq={} ctx={} fork={} exec={} cow={}\n",
        syscalls, irqs, ctx, forks, execs, cow,
    );

    if xhci.total_records > 0 {
        print!(
            "xhci: {} records, last_cc={} ({}) slot={} ep={}\n",
            xhci.total_records,
            xhci.last_xfer_cc,
            cc_name(xhci.last_xfer_cc),
            xhci.last_xfer_slot,
            xhci.last_xfer_ep,
        );
        print!(
            "xhci: {} CMD_COMPLETE {} XFER_EVENT {} NOTE\n",
            xhci.cmd_complete, xhci.xfer_event, xhci.note,
        );
        print!(
            "xhci: poll={} evt={} xe={} rst={} qsrc={} fcc={}\n",
            xhci.poll_count, xhci.event_count,
            xhci.consumed_xfer_enum, xhci.endpoint_resets,
            xhci.first_queue_src, xhci.first_xfer_cc,
        );
        // Dump raw XHCI_DIAG section for new diagnostic fields
        let xhci_text = core::str::from_utf8(&xhci_data).unwrap_or("");
        let mut in_diag = false;
        for line in xhci_text.lines() {
            if line.starts_with("=== XHCI_DIAG ===") {
                in_diag = true;
                continue;
            }
            if line.starts_with("=== XHCI_DIAG_END ===") {
                break;
            }
            if in_diag && !line.starts_with("poll_count")
                && !line.starts_with("event_count")
                && !line.starts_with("consumed_xfer")
                && !line.starts_with("first_xfer_cc ")
                && !line.starts_with("first_queue_src")
                && !line.starts_with("endpoint_resets")
            {
                print!("xhci: {}\n", line);
            }
        }
    } else {
        print!("xhci: no trace records\n");
    }

    print!("=====================\n");
    std::process::exit(0);
}
