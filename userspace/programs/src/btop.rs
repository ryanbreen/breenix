//! btop - Breenix System Monitor
//!
//! A top/htop-like system monitor that reads from procfs and displays
//! live system statistics. Designed to run inside a PTY via bwm.
//!
//! Reads:
//!   /proc/uptime   — system uptime
//!   /proc/meminfo  — memory usage
//!   /proc/stat     — kernel counters (syscalls, IRQs, context switches, etc.)
//!   /proc/pids     — list of process IDs
//!   /proc/<pid>/status — per-process info (name, state, memory, CPU ticks)

use libbreenix::io;
use libbreenix::types::Fd;

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Read a procfs file into a buffer, return the bytes read
fn read_procfs(path: &str, buf: &mut [u8]) -> usize {
    match libbreenix::fs::open(path, 0) {
        Ok(fd) => {
            let mut total = 0;
            loop {
                match io::read(fd, &mut buf[total..]) {
                    Ok(n) if n > 0 => {
                        total += n;
                        if total >= buf.len() - 1 {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            let _ = io::close(fd);
            total
        }
        Err(_) => 0,
    }
}

/// Parse a "key value\n" line from procfs content, return value as u64
fn parse_value(content: &[u8], key: &[u8]) -> u64 {
    // Search for key in content
    let mut i = 0;
    while i + key.len() < content.len() {
        if &content[i..i + key.len()] == key {
            // Skip to value (past whitespace)
            let mut j = i + key.len();
            while j < content.len() && (content[j] == b' ' || content[j] == b'\t') {
                j += 1;
            }
            // Parse number
            let mut val: u64 = 0;
            while j < content.len() && content[j] >= b'0' && content[j] <= b'9' {
                val = val.saturating_mul(10).saturating_add((content[j] - b'0') as u64);
                j += 1;
            }
            return val;
        }
        // Skip to next line
        while i < content.len() && content[i] != b'\n' {
            i += 1;
        }
        if i < content.len() {
            i += 1;
        }
    }
    0
}

/// Parse a "Key:\tValue\n" line from procfs content, return value as string bytes
fn parse_str_value<'a>(content: &'a [u8], key: &[u8]) -> &'a [u8] {
    let mut i = 0;
    while i + key.len() < content.len() {
        if &content[i..i + key.len()] == key {
            let mut j = i + key.len();
            while j < content.len() && (content[j] == b' ' || content[j] == b'\t') {
                j += 1;
            }
            let start = j;
            while j < content.len() && content[j] != b'\n' {
                j += 1;
            }
            return &content[start..j];
        }
        while i < content.len() && content[i] != b'\n' {
            i += 1;
        }
        if i < content.len() {
            i += 1;
        }
    }
    b""
}

/// Format a number with commas: 12345 -> "12,345"
fn format_number(n: u64, buf: &mut [u8]) -> usize {
    if n == 0 {
        buf[0] = b'0';
        return 1;
    }

    // First, write digits in reverse
    let mut tmp = [0u8; 20];
    let mut val = n;
    let mut pos = 0;
    while val > 0 {
        tmp[pos] = b'0' + (val % 10) as u8;
        val /= 10;
        pos += 1;
    }

    // Now write with commas
    let mut out = 0;
    for i in (0..pos).rev() {
        if out > 0 && (pos - 1 - i) > 0 && (pos - 1 - i) % 3 == 0 && i < pos - 1 {
            // Actually, let's simplify: insert comma every 3 digits from the right
        }
        buf[out] = tmp[i];
        out += 1;
    }

    // Redo with proper comma insertion
    out = 0;
    let digits = pos;
    for i in 0..digits {
        let rev_idx = digits - 1 - i;
        buf[out] = tmp[rev_idx];
        out += 1;
        // Insert comma after every 3 digits from the right, except at the end
        let remaining = rev_idx;
        if remaining > 0 && remaining % 3 == 0 {
            buf[out] = b',';
            out += 1;
        }
    }
    out
}

/// Write an ANSI string to stdout
fn emit(s: &[u8]) {
    let _ = io::write(Fd::from_raw(1), s);
}

/// Write a string to stdout
fn emit_str(s: &str) {
    emit(s.as_bytes());
}


/// Write a u64 number to a buffer, return length
fn write_num(buf: &mut [u8], n: u64) -> usize {
    if n == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 20];
    let mut val = n;
    let mut pos = 0;
    while val > 0 {
        tmp[pos] = b'0' + (val % 10) as u8;
        val /= 10;
        pos += 1;
    }
    for i in 0..pos {
        buf[i] = tmp[pos - 1 - i];
    }
    pos
}

/// Print a number to stdout
fn emit_num(n: u64) {
    let mut buf = [0u8; 20];
    let len = write_num(&mut buf, n);
    emit(&buf[..len]);
}

/// Print a formatted number (with commas) to stdout
fn emit_formatted_num(n: u64) {
    let mut buf = [0u8; 30];
    let len = format_number(n, &mut buf);
    emit(&buf[..len]);
}

/// Process info structure
#[derive(Clone)]
struct ProcInfo {
    pid: u64,
    ppid: u64,
    name: [u8; 32],
    name_len: usize,
    state: [u8; 16],
    state_len: usize,
    cpu_ticks: u64,
    vm_heap_kb: u64,
    vm_stack_kb: u64,
    vm_code_kb: u64,
}

impl ProcInfo {
    fn new() -> Self {
        Self {
            pid: 0,
            ppid: 0,
            name: [0; 32],
            name_len: 0,
            state: [0; 16],
            state_len: 0,
            cpu_ticks: 0,
            vm_heap_kb: 0,
            vm_stack_kb: 0,
            vm_code_kb: 0,
        }
    }

    fn total_mem_kb(&self) -> u64 {
        self.vm_code_kb + self.vm_heap_kb + self.vm_stack_kb
    }
}

/// Parse /proc/<pid>/status into a ProcInfo
fn parse_proc_status(pid: u64) -> Option<ProcInfo> {
    let mut path_buf = [0u8; 64];
    let prefix = b"/proc/";
    let suffix = b"/status";
    let mut pos = 0;

    for &b in prefix {
        path_buf[pos] = b;
        pos += 1;
    }
    let mut tmp = [0u8; 20];
    let nlen = write_num(&mut tmp, pid);
    for i in 0..nlen {
        path_buf[pos] = tmp[i];
        pos += 1;
    }
    for &b in suffix {
        path_buf[pos] = b;
        pos += 1;
    }

    let path_str = core::str::from_utf8(&path_buf[..pos]).ok()?;

    let mut buf = [0u8; 512];
    let n = read_procfs(path_str, &mut buf);
    if n == 0 {
        return None;
    }

    let content = &buf[..n];
    let mut info = ProcInfo::new();
    info.pid = pid;

    // Parse Name
    let name_val = parse_str_value(content, b"Name:");
    let copy_len = name_val.len().min(31);
    info.name[..copy_len].copy_from_slice(&name_val[..copy_len]);
    info.name_len = copy_len;

    // Parse PPid
    info.ppid = parse_value(content, b"PPid:");

    // Parse State
    let state_val = parse_str_value(content, b"State:");
    let slen = state_val.len().min(15);
    info.state[..slen].copy_from_slice(&state_val[..slen]);
    info.state_len = slen;

    // Parse CpuTicks
    info.cpu_ticks = parse_value(content, b"CpuTicks:");

    // Parse memory
    info.vm_code_kb = parse_value(content, b"VmCode:");
    info.vm_heap_kb = parse_value(content, b"VmHeap:");
    info.vm_stack_kb = parse_value(content, b"VmStack:");

    Some(info)
}

/// Parse /proc/pids into a list of PIDs
fn get_pids() -> Vec<u64> {
    let mut buf = [0u8; 1024];
    let n = read_procfs("/proc/pids", &mut buf);
    let content = &buf[..n];

    let mut pids = Vec::new();
    let mut val: u64 = 0;
    let mut in_num = false;

    for &b in content {
        if b >= b'0' && b <= b'9' {
            val = val * 10 + (b - b'0') as u64;
            in_num = true;
        } else {
            if in_num {
                pids.push(val);
                val = 0;
                in_num = false;
            }
        }
    }
    if in_num {
        pids.push(val);
    }
    pids
}

/// Draw a memory usage bar
fn draw_memory_bar(used_kb: u64, total_kb: u64) {
    let bar_width = 30;
    let pct = if total_kb > 0 {
        ((used_kb * 100) / total_kb) as usize
    } else {
        0
    };
    let filled = (pct * bar_width) / 100;

    emit_str("\x1b[1m  Memory \x1b[0m[");
    for i in 0..bar_width {
        if i < filled {
            emit_str("\x1b[32m#");
        } else {
            emit_str("\x1b[90m.");
        }
    }
    emit_str("\x1b[0m] ");

    // Show used / total
    let used_mb_int = used_kb / 1024;
    let used_mb_frac = (used_kb % 1024) * 10 / 1024;
    let total_mb_int = total_kb / 1024;
    let total_mb_frac = (total_kb % 1024) * 10 / 1024;

    emit_num(used_mb_int);
    emit_str(".");
    emit_num(used_mb_frac);
    emit_str(" MB / ");
    emit_num(total_mb_int);
    emit_str(".");
    emit_num(total_mb_frac);
    emit_str(" MB (");
    emit_num(pct as u64);
    emit_str("%)");
}

fn main() {
    // Wait a moment for the system to settle
    let _ = libbreenix::time::sleep_ms(500);

    // Previous tick counts for CPU% delta computation
    let mut prev_ticks: Vec<(u64, u64)> = Vec::new(); // (pid, ticks)
    let mut prev_timer_ticks: u64 = 0;

    loop {
        // ── Gather Data ──────────────────────────────────────────────────

        // Uptime
        let mut uptime_buf = [0u8; 64];
        let uptime_n = read_procfs("/proc/uptime", &mut uptime_buf);
        let uptime_secs = parse_value(&uptime_buf[..uptime_n], b""); // First number

        // Actually parse uptime properly - it's "123.45 0.00\n"
        let mut up_secs: u64 = 0;
        for &b in &uptime_buf[..uptime_n] {
            if b >= b'0' && b <= b'9' {
                up_secs = up_secs * 10 + (b - b'0') as u64;
            } else {
                break;
            }
        }
        let _ = uptime_secs; // use up_secs instead

        // Memory
        let mut meminfo_buf = [0u8; 1024];
        let meminfo_n = read_procfs("/proc/meminfo", &mut meminfo_buf);
        let meminfo = &meminfo_buf[..meminfo_n];
        let total_kb = parse_value(meminfo, b"MemTotal:");
        let free_kb = parse_value(meminfo, b"MemFree:");
        let used_kb = total_kb.saturating_sub(free_kb);

        // Kernel counters
        let mut stat_buf = [0u8; 512];
        let stat_n = read_procfs("/proc/stat", &mut stat_buf);
        let stat = &stat_buf[..stat_n];
        let syscalls = parse_value(stat, b"syscalls");
        let interrupts = parse_value(stat, b"interrupts");
        let ctx_switches = parse_value(stat, b"context_switches");
        let timer_ticks = parse_value(stat, b"timer_ticks");
        let forks = parse_value(stat, b"forks");
        let execs = parse_value(stat, b"execs");
        let cow_faults = parse_value(stat, b"cow_faults");

        // Process list
        let pids = get_pids();
        let mut procs: Vec<ProcInfo> = Vec::new();
        for &pid in &pids {
            if let Some(info) = parse_proc_status(pid) {
                procs.push(info);
            }
        }

        // Compute CPU% deltas
        let tick_delta = timer_ticks.saturating_sub(prev_timer_ticks);
        let mut cpu_pcts: Vec<(u64, u64)> = Vec::new(); // (pid, pct*10 for 1 decimal)
        for proc in &procs {
            let prev = prev_ticks.iter().find(|(p, _)| *p == proc.pid);
            let prev_t = prev.map(|(_, t)| *t).unwrap_or(0);
            let delta = proc.cpu_ticks.saturating_sub(prev_t);
            let pct10 = if tick_delta > 0 {
                (delta * 1000) / tick_delta
            } else {
                0
            };
            cpu_pcts.push((proc.pid, pct10));
        }

        // Save current ticks for next iteration
        prev_ticks.clear();
        for proc in &procs {
            prev_ticks.push((proc.pid, proc.cpu_ticks));
        }
        prev_timer_ticks = timer_ticks;

        // ── Render ───────────────────────────────────────────────────────

        // Clear screen and home cursor
        emit_str("\x1b[2J\x1b[H");

        // Header
        emit_str("\x1b[1;36mbtop\x1b[0m - Breenix System Monitor");

        // Uptime (right-aligned)
        emit_str("              Uptime: ");
        let hours = up_secs / 3600;
        let mins = (up_secs % 3600) / 60;
        let secs = up_secs % 60;
        emit_num(hours);
        emit_str(":");
        if mins < 10 { emit_str("0"); }
        emit_num(mins);
        emit_str(":");
        if secs < 10 { emit_str("0"); }
        emit_num(secs);
        emit_str("\n\n");

        // Memory bar
        draw_memory_bar(used_kb, total_kb);
        emit_str("\n\n");

        // Process table header
        emit_str("\x1b[1m  PID  PPID  STATE        CPU%     MEM     NAME\x1b[0m\n");
        emit_str(" ---- ----- ---------- ------ -------- ----------------\n");

        // Sort by CPU% descending
        // Simple insertion sort since we have few processes
        let mut sorted_indices: Vec<usize> = (0..procs.len()).collect();
        for i in 1..sorted_indices.len() {
            let mut j = i;
            while j > 0 {
                let a = sorted_indices[j];
                let b = sorted_indices[j - 1];
                let pct_a = cpu_pcts.iter().find(|(p, _)| *p == procs[a].pid).map(|(_, p)| *p).unwrap_or(0);
                let pct_b = cpu_pcts.iter().find(|(p, _)| *p == procs[b].pid).map(|(_, p)| *p).unwrap_or(0);
                if pct_a > pct_b {
                    sorted_indices.swap(j, j - 1);
                    j -= 1;
                } else {
                    break;
                }
            }
        }

        for &idx in &sorted_indices {
            let proc = &procs[idx];
            let pct10 = cpu_pcts.iter().find(|(p, _)| *p == proc.pid).map(|(_, p)| *p).unwrap_or(0);

            // Color based on state
            let state_bytes = &proc.state[..proc.state_len];
            if state_bytes == b"Running" {
                emit_str("\x1b[32m"); // Green
            } else if state_bytes == b"Blocked" {
                emit_str("\x1b[33m"); // Yellow
            } else if state_bytes.starts_with(b"Terminated") {
                emit_str("\x1b[31m"); // Red
            }

            // PID (right-aligned in 5 chars)
            emit_str("  ");
            if proc.pid < 10 { emit_str("   "); }
            else if proc.pid < 100 { emit_str("  "); }
            else if proc.pid < 1000 { emit_str(" "); }
            emit_num(proc.pid);

            // PPID
            emit_str("  ");
            if proc.ppid < 10 { emit_str("   "); }
            else if proc.ppid < 100 { emit_str("  "); }
            else if proc.ppid < 1000 { emit_str(" "); }
            emit_num(proc.ppid);

            // State (padded to 10 chars)
            emit_str("  ");
            emit(&proc.state[..proc.state_len]);
            for _ in proc.state_len..10 {
                emit_str(" ");
            }

            // CPU% (e.g., "  1.5%")
            emit_str(" ");
            let pct_int = pct10 / 10;
            let pct_frac = pct10 % 10;
            if pct_int < 10 { emit_str("  "); }
            else if pct_int < 100 { emit_str(" "); }
            emit_num(pct_int);
            emit_str(".");
            emit_num(pct_frac);
            emit_str("%");

            // Memory
            emit_str("  ");
            let mem = proc.total_mem_kb();
            if mem < 10 { emit_str("     "); }
            else if mem < 100 { emit_str("    "); }
            else if mem < 1000 { emit_str("   "); }
            else if mem < 10000 { emit_str("  "); }
            else if mem < 100000 { emit_str(" "); }
            emit_num(mem);
            emit_str(" kB");

            // Name
            emit_str("  ");
            emit(&proc.name[..proc.name_len]);

            emit_str("\x1b[0m\n");
        }

        // Footer with kernel counters
        emit_str("\n");
        emit_str("  Syscalls: ");
        emit_formatted_num(syscalls);
        emit_str("  |  IRQs: ");
        emit_formatted_num(interrupts);
        emit_str("  |  Ctx Sw: ");
        emit_formatted_num(ctx_switches);
        emit_str("\n");
        emit_str("  Forks: ");
        emit_formatted_num(forks);
        emit_str("       |  Execs: ");
        emit_formatted_num(execs);
        emit_str("    |  CoW: ");
        emit_formatted_num(cow_faults);
        emit_str("\n");

        // Sleep 1 second before next refresh
        let _ = libbreenix::time::sleep_ms(1000);
    }
}
