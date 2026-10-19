//! Host keys over the serial console, for x86-metal suite boots
//! (docs/console/contract.md section 10, "Host-injected keys").
//!
//! The bare-metal boot loop has no virtual keyboard it may press: it reaches
//! the machine through power control and the serial console only. A suite
//! boot whose target declares `hostkeys xhci-replay` takes the boot loop's
//! answers to its HOSTKEY records as serial lines,
//!
//! ```text
//! BXKEY <report>[,<report>...]
//! ```
//!
//! each report sixteen hex digits: one HID boot keyboard report (HID 1.11
//! B.1: modifier bits, a reserved byte, six usages). The xHCI worker replays
//! the reports in order through the keyboard's interrupt-IN completion path
//! (`Controller::replay_key`), the path a report takes once the device has
//! written it. While the path is armed, serial input is this channel only.
//!
//! This file is the line format, kept free of kernel state so the host can
//! test it (`rustc --edition=2021 --test`).

/// Bytes in one HID boot keyboard report.
pub const REPORT_LEN: usize = 8;
/// Reports one line may carry: a chord holding the six modifiers, pressed and
/// released, is fourteen.
pub const MAX_REPORTS: usize = 16;
/// The longest line: the prefix and `MAX_REPORTS` reports, comma-separated.
pub const MAX_LINE: usize = PREFIX.len() + MAX_REPORTS * (2 * REPORT_LEN + 1) - 1;

const PREFIX: &[u8] = b"BXKEY ";

/// One line's reports, in the order they are replayed.
pub struct Reports {
    reports: [[u8; REPORT_LEN]; MAX_REPORTS],
    count: usize,
}

impl Reports {
    pub fn as_slice(&self) -> &[[u8; REPORT_LEN]] {
        &self.reports[..self.count]
    }
}

/// Parse one line, without its terminator.
pub fn parse(line: &[u8]) -> Result<Reports, &'static str> {
    let body = line.strip_prefix(PREFIX).ok_or("not a BXKEY line")?;
    let mut parsed = Reports { reports: [[0; REPORT_LEN]; MAX_REPORTS], count: 0 };
    for field in body.split(|&b| b == b',') {
        if parsed.count == MAX_REPORTS {
            return Err("more reports than one line may carry");
        }
        if field.len() != 2 * REPORT_LEN {
            return Err("a report is sixteen hex digits");
        }
        let report = &mut parsed.reports[parsed.count];
        for (byte, pair) in report.iter_mut().zip(field.chunks_exact(2)) {
            *byte = hex(pair[0])? << 4 | hex(pair[1])?;
        }
        parsed.count += 1;
    }
    Ok(parsed)
}

fn hex(digit: u8) -> Result<u8, &'static str> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        _ => Err("a report is lowercase hex"),
    }
}

/// Serial bytes gathered into lines. A line ends at CR or LF; an empty line
/// is skipped, and a line longer than `MAX_LINE` is refused whole.
pub struct Lines {
    line: [u8; MAX_LINE],
    len: usize,
    overlong: bool,
}

impl Lines {
    pub const fn new() -> Self {
        Self { line: [0; MAX_LINE], len: 0, overlong: false }
    }

    /// Take one byte; at a line's end, that line's reports.
    pub fn feed(&mut self, byte: u8) -> Option<Result<Reports, &'static str>> {
        if byte != b'\r' && byte != b'\n' {
            if self.len == MAX_LINE {
                self.overlong = true;
            } else {
                self.line[self.len] = byte;
                self.len += 1;
            }
            return None;
        }
        let (len, overlong) = (self.len, self.overlong);
        self.len = 0;
        self.overlong = false;
        match (len, overlong) {
            (0, false) => None,
            (_, true) => Some(Err("line longer than a BXKEY line")),
            _ => Some(parse(&self.line[..len])),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(input: &[u8]) -> std::vec::Vec<Result<std::vec::Vec<[u8; 8]>, &'static str>> {
        let mut reader = Lines::new();
        input.iter().filter_map(|&b| reader.feed(b)).map(|r| r.map(|r| r.as_slice().to_vec())).collect()
    }

    #[test]
    fn a_chord_is_its_reports_in_order() {
        let got = lines(b"BXKEY 0400000000000000,04003b0000000000,0400000000000000,0000000000000000\r");
        assert_eq!(got, [Ok(std::vec![
            [4, 0, 0, 0, 0, 0, 0, 0],
            [4, 0, 0x3b, 0, 0, 0, 0, 0],
            [4, 0, 0, 0, 0, 0, 0, 0],
            [0; 8],
        ])]);
    }

    #[test]
    fn crlf_and_blank_lines_give_one_line_each() {
        let got = lines(b"\r\nBXKEY 0000040000000000\r\n\nBXKEY 0000000000000000\n");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], Ok(std::vec![[0, 0, 4, 0, 0, 0, 0, 0]]));
    }

    #[test]
    fn malformed_lines_are_refused() {
        for bad in [&b"bxkey 0000000000000000"[..], b"BXKEY ", b"BXKEY 000000000000000", b"BXKEY 00000000000000000",
                    b"BXKEY 000000000000000G", b"BXKEY 00000000000000AA", b"BXKEY 0000000000000000,",
                    b"BXKEY 0000000000000000 ", b"breenix> help"] {
            assert!(parse(bad).is_err(), "{:?}", core::str::from_utf8(bad));
        }
    }

    #[test]
    fn report_count_and_line_length_are_bounded() {
        let mut line = std::vec::Vec::from(PREFIX);
        for i in 0..MAX_REPORTS {
            if i != 0 {
                line.push(b',');
            }
            line.extend_from_slice(b"0000000000000000");
        }
        assert_eq!(line.len(), MAX_LINE);
        assert_eq!(parse(&line).map(|r| r.as_slice().len()), Ok(MAX_REPORTS));
        line.extend_from_slice(b",0000000000000000");
        assert!(parse(&line).is_err());
        line.push(b'\r');
        let got = lines(&line);
        assert_eq!(got, [Err("line longer than a BXKEY line")]);
        // The reader recovers at the next line.
        assert_eq!(lines(&[&line[..], b"BXKEY 0000000000000000\r"].concat()).len(), 2);
    }
}
