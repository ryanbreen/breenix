//! bless — Breenix Less (interactive log pager)
//!
//! An interactive terminal pager (like `less`) designed to run inside BWM's
//! PTY tab. Defaults to /var/log/kernel.log with follow mode when invoked
//! with no arguments.
//!
//! Features:
//!   - Line-based viewport with scroll
//!   - Follow mode: auto-scroll as file grows (detect via fstat size check)
//!   - Raw terminal mode via tcsetattr
//!   - ANSI rendering: clear screen, cursor positioning, colors
//!   - Status bar at bottom: filename, line position, percentage
//!
//! Navigation:
//!   ↑/k       Scroll up one line
//!   ↓/j       Scroll down one line
//!   PgUp/b    Page up
//!   PgDn/Space Page down
//!   g/Home    Go to top
//!   G/End     Go to bottom
//!   f         Toggle follow mode
//!   /         Search forward (enter pattern, then Enter)
//!   n         Next search match
//!   q         Quit

use libbreenix::fs;
use libbreenix::io;
use libbreenix::termios;
use libbreenix::time;
use libbreenix::types::{Fd, Timespec};

// ─── Constants ──────────────────────────────────────────────────────────────

const DEFAULT_FILE: &str = "/var/log/kernel.log";
const READ_BUF_SIZE: usize = 8192;
const MAX_FILE_SIZE: usize = 256 * 1024; // 256KB max we'll buffer

// ANSI escape sequences
const CSI_CLEAR_SCREEN: &[u8] = b"\x1b[2J";
const CSI_HOME: &[u8] = b"\x1b[H";
const CSI_HIDE_CURSOR: &[u8] = b"\x1b[?25l";
const CSI_SHOW_CURSOR: &[u8] = b"\x1b[?25h";
const CSI_REVERSE: &[u8] = b"\x1b[7m";
const CSI_RESET: &[u8] = b"\x1b[0m";
const CSI_DIM: &[u8] = b"\x1b[2m";
const CSI_YELLOW_FG: &[u8] = b"\x1b[33m";
const CSI_CYAN_FG: &[u8] = b"\x1b[36m";
const CSI_ERASE_LINE: &[u8] = b"\x1b[K";

// ─── Terminal Dimensions ────────────────────────────────────────────────────

struct TermSize {
    rows: usize,
    cols: usize,
}

fn get_term_size() -> TermSize {
    match termios::get_winsize(Fd::from_raw(0)) {
        Ok(ws) if ws.ws_row > 0 && ws.ws_col > 0 => TermSize {
            rows: ws.ws_row as usize,
            cols: ws.ws_col as usize,
        },
        _ => TermSize { rows: 24, cols: 80 }, // Fallback
    }
}

// ─── Raw Terminal Mode ──────────────────────────────────────────────────────

struct RawMode {
    orig: termios::Termios,
}

impl RawMode {
    fn enter() -> Self {
        let mut orig = termios::Termios::default();
        let _ = termios::tcgetattr(Fd::from_raw(0), &mut orig);
        let mut raw = orig;
        termios::cfmakeraw(&mut raw);
        // Set VMIN=0 VTIME=1 for non-blocking reads with 100ms timeout
        raw.c_cc[termios::cc::VMIN] = 0;
        raw.c_cc[termios::cc::VTIME] = 1;
        let _ = termios::tcsetattr(Fd::from_raw(0), termios::TCSANOW, &raw);
        let _ = io::write(Fd::from_raw(1), CSI_HIDE_CURSOR);
        RawMode { orig }
    }

    fn restore(&self) {
        let _ = io::write(Fd::from_raw(1), CSI_SHOW_CURSOR);
        let _ = io::write(Fd::from_raw(1), CSI_RESET);
        let _ = termios::tcsetattr(Fd::from_raw(0), termios::TCSANOW, &self.orig);
    }
}

// ─── Output Helpers ─────────────────────────────────────────────────────────

fn write_stdout(data: &[u8]) {
    let _ = io::write(Fd::from_raw(1), data);
}

fn write_str(s: &str) {
    write_stdout(s.as_bytes());
}

/// Move cursor to row, col (1-based)
fn move_cursor(row: usize, col: usize) {
    let mut buf = [0u8; 16];
    let len = format_csi_pos(&mut buf, row, col);
    write_stdout(&buf[..len]);
}

/// Format ESC[row;colH into buffer, return length
fn format_csi_pos(buf: &mut [u8; 16], row: usize, col: usize) -> usize {
    buf[0] = b'\x1b';
    buf[1] = b'[';
    let mut pos = 2;
    pos += write_num(&mut buf[pos..], row);
    buf[pos] = b';';
    pos += 1;
    pos += write_num(&mut buf[pos..], col);
    buf[pos] = b'H';
    pos + 1
}

/// Write a number as ASCII digits, return bytes written
fn write_num(buf: &mut [u8], mut n: usize) -> usize {
    if n == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 10];
    let mut count = 0;
    while n > 0 {
        digits[count] = b'0' + (n % 10) as u8;
        n /= 10;
        count += 1;
    }
    for i in 0..count {
        buf[i] = digits[count - 1 - i];
    }
    count
}

// ─── File Loading ───────────────────────────────────────────────────────────

/// Load file content into a Vec. Returns empty Vec if file doesn't exist.
fn load_file(path: &str) -> Vec<u8> {
    let mut content = Vec::new();
    match fs::open(path, fs::O_RDONLY) {
        Ok(fd) => {
            let mut buf = [0u8; READ_BUF_SIZE];
            loop {
                match io::read(fd, &mut buf) {
                    Ok(n) if n > 0 => {
                        if content.len() + n > MAX_FILE_SIZE {
                            // Truncate to max size, keeping the tail
                            let keep = MAX_FILE_SIZE.saturating_sub(n);
                            if keep < content.len() {
                                let drain = content.len() - keep;
                                content.drain(..drain);
                            }
                        }
                        content.extend_from_slice(&buf[..n]);
                    }
                    _ => break,
                }
            }
            let _ = io::close(fd);
        }
        Err(_) => {}
    }
    content
}

/// Get file size via fstat. Returns -1 if the file doesn't exist/can't be opened.
fn file_size(path: &str) -> i64 {
    match fs::open(path, fs::O_RDONLY) {
        Ok(fd) => {
            let size = match fs::fstat(fd) {
                Ok(stat) => stat.st_size,
                Err(_) => -1,
            };
            let _ = io::close(fd);
            size
        }
        Err(_) => -1,
    }
}

// ─── Line Index ─────────────────────────────────────────────────────────────

/// Build a line index: Vec of byte offsets where each line starts.
fn build_line_index(content: &[u8]) -> Vec<usize> {
    let mut lines = vec![0]; // First line starts at 0
    for (i, &b) in content.iter().enumerate() {
        if b == b'\n' && i + 1 < content.len() {
            lines.push(i + 1);
        }
    }
    lines
}

/// Get the byte slice for a given line number.
fn get_line<'a>(content: &'a [u8], line_starts: &[usize], line: usize) -> &'a [u8] {
    if line >= line_starts.len() {
        return b"";
    }
    let start = line_starts[line];
    let end = if line + 1 < line_starts.len() {
        let e = line_starts[line + 1];
        // Strip trailing newline
        if e > 0 && content[e - 1] == b'\n' {
            e - 1
        } else {
            e
        }
    } else {
        content.len()
    };
    if start >= content.len() {
        return b"";
    }
    &content[start..end.min(content.len())]
}

// ─── Input Parsing ──────────────────────────────────────────────────────────

enum Input {
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    GoTop,
    GoBottom,
    ToggleFollow,
    SearchStart,
    SearchNext,
    Quit,
    Other,
    None,
}

/// Parse input from a byte buffer (may contain escape sequences).
fn parse_input(buf: &[u8], len: usize) -> Input {
    if len == 0 {
        return Input::None;
    }

    // Single character
    if len == 1 {
        return match buf[0] {
            b'q' | b'Q' => Input::Quit,
            b'k' => Input::ScrollUp,
            b'j' => Input::ScrollDown,
            b'b' => Input::PageUp,
            b' ' => Input::PageDown,
            b'g' => Input::GoTop,
            b'G' => Input::GoBottom,
            b'f' => Input::ToggleFollow,
            b'/' => Input::SearchStart,
            b'n' => Input::SearchNext,
            _ => Input::Other,
        };
    }

    // Escape sequences
    if len >= 3 && buf[0] == 0x1b && buf[1] == b'[' {
        match buf[2] {
            b'A' => return Input::ScrollUp,   // Up arrow
            b'B' => return Input::ScrollDown,  // Down arrow
            b'H' => return Input::GoTop,       // Home
            b'F' => return Input::GoBottom,    // End
            _ => {}
        }
        // CSI sequences with numbers: ESC [ N ~
        if len >= 4 && buf[3] == b'~' {
            match buf[2] {
                b'5' => return Input::PageUp,   // PgUp
                b'6' => return Input::PageDown,  // PgDn
                b'1' => return Input::GoTop,     // Home (alt)
                b'4' => return Input::GoBottom,  // End (alt)
                _ => {}
            }
        }
    }

    Input::None
}

// ─── Search ─────────────────────────────────────────────────────────────────

/// Find the next line containing the search pattern, starting from `from_line`.
fn find_next_match(
    content: &[u8],
    line_starts: &[usize],
    pattern: &[u8],
    from_line: usize,
) -> Option<usize> {
    if pattern.is_empty() {
        return None;
    }
    for line_num in from_line..line_starts.len() {
        let line = get_line(content, line_starts, line_num);
        if contains_bytes(line, pattern) {
            return Some(line_num);
        }
    }
    // Wrap around from the beginning
    for line_num in 0..from_line {
        let line = get_line(content, line_starts, line_num);
        if contains_bytes(line, pattern) {
            return Some(line_num);
        }
    }
    None
}

/// Check if `haystack` contains `needle` (case-insensitive byte search).
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.len() > haystack.len() {
        return false;
    }
    'outer: for i in 0..=(haystack.len() - needle.len()) {
        for j in 0..needle.len() {
            let h = to_lower(haystack[i + j]);
            let n = to_lower(needle[j]);
            if h != n {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

fn to_lower(b: u8) -> u8 {
    if b >= b'A' && b <= b'Z' {
        b + 32
    } else {
        b
    }
}

// ─── Rendering ──────────────────────────────────────────────────────────────

fn render(
    content: &[u8],
    line_starts: &[usize],
    scroll_pos: usize,
    term: &TermSize,
    filename: &str,
    follow_mode: bool,
    search_pattern: &[u8],
) {
    write_stdout(CSI_HOME);

    let viewport_rows = term.rows.saturating_sub(1); // Reserve 1 row for status bar
    let total_lines = line_starts.len();

    for row in 0..viewport_rows {
        let line_num = scroll_pos + row;
        move_cursor(row + 1, 1);
        write_stdout(CSI_ERASE_LINE);

        if line_num < total_lines {
            let line = get_line(content, line_starts, line_num);
            // Truncate to terminal width
            let display_len = line.len().min(term.cols);
            let display = &line[..display_len];

            // Highlight search matches
            if !search_pattern.is_empty() && contains_bytes(display, search_pattern) {
                render_highlighted_line(display, search_pattern, term.cols);
            } else {
                write_stdout(display);
            }
        } else {
            // Empty line indicator
            write_stdout(CSI_DIM);
            write_stdout(b"~");
            write_stdout(CSI_RESET);
        }
    }

    // Status bar at bottom
    render_status_bar(term, filename, scroll_pos, total_lines, viewport_rows, follow_mode);
}

/// Render a line with search matches highlighted.
fn render_highlighted_line(line: &[u8], pattern: &[u8], max_cols: usize) {
    let display_len = line.len().min(max_cols);
    let display = &line[..display_len];
    let plen = pattern.len();
    let mut i = 0;

    while i < display.len() {
        if i + plen <= display.len() && contains_bytes(&display[i..i + plen], pattern) {
            write_stdout(CSI_REVERSE);
            write_stdout(CSI_YELLOW_FG);
            write_stdout(&display[i..i + plen]);
            write_stdout(CSI_RESET);
            i += plen;
        } else {
            write_stdout(&display[i..i + 1]);
            i += 1;
        }
    }
}

/// Render the status bar at the bottom of the screen.
fn render_status_bar(
    term: &TermSize,
    filename: &str,
    scroll_pos: usize,
    total_lines: usize,
    viewport_rows: usize,
    follow_mode: bool,
) {
    move_cursor(term.rows, 1);
    write_stdout(CSI_REVERSE);
    write_stdout(CSI_ERASE_LINE);

    // Left: filename
    write_stdout(b" ");
    write_str(filename);

    // Middle: line position
    write_stdout(b"  L:");
    let mut num_buf = [0u8; 16];
    let len = write_num(&mut num_buf, scroll_pos + 1);
    write_stdout(&num_buf[..len]);
    write_stdout(b"-");
    let end_line = (scroll_pos + viewport_rows).min(total_lines);
    let len = write_num(&mut num_buf, end_line);
    write_stdout(&num_buf[..len]);
    write_stdout(b"/");
    let len = write_num(&mut num_buf, total_lines);
    write_stdout(&num_buf[..len]);

    // Right: percentage and follow indicator
    if total_lines > 0 {
        let pct = if total_lines <= viewport_rows {
            100
        } else if scroll_pos + viewport_rows >= total_lines {
            100
        } else {
            ((scroll_pos + viewport_rows) * 100) / total_lines
        };
        write_stdout(b"  ");
        let len = write_num(&mut num_buf, pct);
        write_stdout(&num_buf[..len]);
        write_stdout(b"%");
    }

    if follow_mode {
        write_stdout(b"  [FOLLOW]");
    }

    write_stdout(CSI_RESET);
}

/// Render the search prompt at the bottom.
fn render_search_prompt(term: &TermSize, pattern: &[u8]) {
    move_cursor(term.rows, 1);
    write_stdout(CSI_REVERSE);
    write_stdout(CSI_ERASE_LINE);
    write_stdout(b" /");
    write_stdout(pattern);
    write_stdout(CSI_SHOW_CURSOR);
    write_stdout(CSI_RESET);
}

/// Show a "waiting for file" message centered on screen.
fn render_waiting(term: &TermSize, filename: &str) {
    write_stdout(CSI_HOME);
    write_stdout(CSI_CLEAR_SCREEN);

    let msg_row = term.rows / 2;
    move_cursor(msg_row, 1);
    write_stdout(CSI_DIM);
    write_stdout(CSI_CYAN_FG);
    write_str("  Waiting for ");
    write_str(filename);
    write_str("...");
    write_stdout(CSI_RESET);

    move_cursor(msg_row + 2, 1);
    write_stdout(CSI_DIM);
    write_str("  Press q to quit");
    write_stdout(CSI_RESET);
}

// ─── Main ───────────────────────────────────────────────────────────────────

fn main() {
    // Parse args: bless [filename]
    let args: Vec<String> = std::env::args().collect();
    let filename = if args.len() > 1 {
        args[1].as_str()
    } else {
        DEFAULT_FILE
    };
    // If no explicit file given, default to follow mode
    let default_follow = args.len() <= 1;

    let raw_mode = RawMode::enter();
    write_stdout(CSI_CLEAR_SCREEN);

    let mut term = get_term_size();
    let mut scroll_pos: usize = 0;
    let mut follow_mode = default_follow;
    let mut search_pattern: Vec<u8> = Vec::new();
    let mut in_search_mode = false;
    let mut search_buf: Vec<u8> = Vec::new();
    let mut last_file_size: i64 = 0;

    let mut content = load_file(filename);
    let mut line_starts = build_line_index(&content);

    // If file doesn't exist, show waiting screen
    let mut file_exists = !content.is_empty() || file_size(filename) >= 0;
    // Also initialize last_file_size from actual content loaded
    if file_exists {
        last_file_size = content.len() as i64;
    }

    if file_exists && follow_mode && !line_starts.is_empty() {
        // Start scrolled to bottom in follow mode
        let viewport_rows = term.rows.saturating_sub(1);
        if line_starts.len() > viewport_rows {
            scroll_pos = line_starts.len() - viewport_rows;
        }
    }

    if file_exists {
        render(
            &content,
            &line_starts,
            scroll_pos,
            &term,
            filename,
            follow_mode,
            &search_pattern,
        );
    } else {
        render_waiting(&term, filename);
    }

    let mut input_buf = [0u8; 32];
    let poll_sleep = Timespec {
        tv_sec: 0,
        tv_nsec: 100_000_000, // 100ms
    };
    let mut refresh_counter: u32 = 0;

    loop {
        // Read input (non-blocking via VTIME)
        let n = io::read(Fd::from_raw(0), &mut input_buf).unwrap_or(0);

        if in_search_mode {
            // Handle search input
            for i in 0..n {
                match input_buf[i] {
                    b'\r' | b'\n' => {
                        // Commit search
                        in_search_mode = false;
                        search_pattern = search_buf.clone();
                        write_stdout(CSI_HIDE_CURSOR);
                        // Find first match from current position
                        if !search_pattern.is_empty() {
                            if let Some(line) = find_next_match(
                                &content,
                                &line_starts,
                                &search_pattern,
                                scroll_pos,
                            ) {
                                scroll_pos = line;
                                follow_mode = false;
                            }
                        }
                        break;
                    }
                    0x1b => {
                        // Escape: cancel search
                        in_search_mode = false;
                        search_buf.clear();
                        write_stdout(CSI_HIDE_CURSOR);
                        break;
                    }
                    0x7f | 0x08 => {
                        // Backspace
                        search_buf.pop();
                    }
                    ch if ch >= 0x20 => {
                        search_buf.push(ch);
                    }
                    _ => {}
                }
            }

            if in_search_mode {
                render_search_prompt(&term, &search_buf);
                continue;
            }
        } else if n > 0 {
            let input = parse_input(&input_buf, n);
            let viewport_rows = term.rows.saturating_sub(1);
            let total_lines = line_starts.len();

            match input {
                Input::Quit => break,
                Input::ScrollUp => {
                    if scroll_pos > 0 {
                        scroll_pos -= 1;
                        follow_mode = false;
                    }
                }
                Input::ScrollDown => {
                    if scroll_pos + viewport_rows < total_lines {
                        scroll_pos += 1;
                    }
                }
                Input::PageUp => {
                    scroll_pos = scroll_pos.saturating_sub(viewport_rows);
                    follow_mode = false;
                }
                Input::PageDown => {
                    let max = total_lines.saturating_sub(viewport_rows);
                    scroll_pos = (scroll_pos + viewport_rows).min(max);
                }
                Input::GoTop => {
                    scroll_pos = 0;
                    follow_mode = false;
                }
                Input::GoBottom => {
                    let max = total_lines.saturating_sub(viewport_rows);
                    scroll_pos = max;
                }
                Input::ToggleFollow => {
                    follow_mode = !follow_mode;
                    if follow_mode {
                        // Jump to bottom
                        let max = total_lines.saturating_sub(viewport_rows);
                        scroll_pos = max;
                    }
                }
                Input::SearchStart => {
                    in_search_mode = true;
                    search_buf.clear();
                    render_search_prompt(&term, &search_buf);
                    continue;
                }
                Input::SearchNext => {
                    if !search_pattern.is_empty() {
                        let from = if scroll_pos + 1 < total_lines {
                            scroll_pos + 1
                        } else {
                            0
                        };
                        if let Some(line) = find_next_match(
                            &content,
                            &line_starts,
                            &search_pattern,
                            from,
                        ) {
                            scroll_pos = line;
                            follow_mode = false;
                        }
                    }
                }
                Input::Other | Input::None => {}
            }
        }

        // Periodic: check for file growth (every ~500ms = 5 iterations)
        refresh_counter += 1;
        if refresh_counter % 5 == 0 {
            // Refresh terminal size
            term = get_term_size();

            if !file_exists {
                // Check if file appeared
                let sz = file_size(filename);
                if sz > 0 {
                    file_exists = true;
                    content = load_file(filename);
                    line_starts = build_line_index(&content);
                    last_file_size = sz;
                    if follow_mode && !line_starts.is_empty() {
                        let viewport_rows = term.rows.saturating_sub(1);
                        if line_starts.len() > viewport_rows {
                            scroll_pos = line_starts.len() - viewport_rows;
                        }
                    }
                } else {
                    render_waiting(&term, filename);
                    let _ = time::nanosleep(&poll_sleep);
                    continue;
                }
            } else {
                // Check for file growth
                let current_size = file_size(filename);
                if current_size != last_file_size {
                    last_file_size = current_size;
                    content = load_file(filename);
                    line_starts = build_line_index(&content);

                    if follow_mode {
                        let viewport_rows = term.rows.saturating_sub(1);
                        let max = line_starts.len().saturating_sub(viewport_rows);
                        scroll_pos = max;
                    }
                }
            }
        }

        // Render
        if file_exists {
            render(
                &content,
                &line_starts,
                scroll_pos,
                &term,
                filename,
                follow_mode,
                &search_pattern,
            );
        }

        // Small sleep if no input to avoid busy-waiting
        if n == 0 {
            let _ = time::nanosleep(&poll_sleep);
        }
    }

    // Restore terminal
    raw_mode.restore();
    write_stdout(CSI_CLEAR_SCREEN);
    write_stdout(CSI_HOME);
}
