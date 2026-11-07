//! Mandatory GUI result records (docs/gui/plan.md sections 8.2–8.3).
//!
//! This module parses claims, binds them to the active case and accounts for declared
//! frame checks. It does not inspect images. The runner owns the pipe and serial output.

use std::string::String;
use std::vec::Vec;

/// Includes the terminating newline, so one blocking write fits PIPE_BUF.
pub const MAX_RECORD_BYTES: usize = 4096;
pub const RECORDS_PER_CASE: usize = 256;

fn name(text: &str) -> bool {
    !text.is_empty() && text.split('-').all(|part| {
        !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

fn case_name(text: &str) -> bool {
    text.split_once('/').is_some_and(|(category, case)| name(category) && name(case))
}

fn hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn number(text: &str) -> Option<u64> {
    if text.is_empty() || text.len() > 20 || !text.bytes().all(|b| b.is_ascii_digit()) { return None; }
    text.parse().ok()
}

fn field<'a>(token: &'a str, key: &str) -> Option<&'a str> {
    token.strip_prefix(key)?.strip_prefix('=')
}

fn count(token: &str, key: &str) -> Option<u64> { number(field(token, key)?) }

fn artifacts(text: &str) -> bool {
    text == "complete" || text.split_once(':').is_some_and(|(kind, reason)| {
        matches!(kind, "partial" | "none") && name(reason)
    })
}

#[derive(Clone, Copy)]
enum Record<'a> {
    Frame { check: &'a str, matched: bool },
    Tree(&'a str),
    Input,
    Collect,
}

fn parse<'a>(line: &'a str, suite: &str, case: &str) -> Option<Record<'a>> {
    if line.len() + 1 > MAX_RECORD_BYTES || !line.is_ascii() { return None; }
    // Single spaces, no controls or trailing fields: serial and pipe use the same grammar.
    let t: Vec<&str> = line.split(' ').collect();
    if t.len() < 4 || t.iter().any(|s| s.is_empty() || s.bytes().any(|b| b <= b' ' || b == 127))
        || t[0] != "SUITE" || t[1] != suite || t[3] != case { return None; }
    match t[2] {
        "FRAME" => {
            if !matches!(t.len(), 14 | 20) || !name(t[4]) || !hex(field(t[5], "def")?, 8) { return None; }
            let reference = field(t[6], "ref")?;
            if !matches!(reference, "formula" | "structural") && !hex(reference, 8) { return None; }
            let source = field(t[7], "source")?;
            if !matches!(source, "screen" | "surface") && !source.strip_prefix("window:").is_some_and(name) { return None; }
            count(t[8], "commit")?;
            count(t[9], "epoch")?;
            let w = count(t[10], "w")?;
            let h = count(t[11], "h")?;
            if w == 0 || h == 0 || !hex(field(t[12], "sha256")?, 64) { return None; }
            let matched = field(t[13], "result")?;
            match matched {
                "match" if t.len() == 14 => Some(Record::Frame { check: t[4], matched: true }),
                "mismatch" if t.len() == 20 => {
                    let compared = count(t[14], "compared")?;
                    let differing = count(t[15], "differing")?;
                    let (x, y) = field(t[16], "first")?.split_once(',')?;
                    if compared == 0 || compared > w.checked_mul(h)? || differing == 0 || differing > compared
                        || number(x)? >= w || number(y)? >= h
                        || !hex(field(t[17], "got")?.strip_prefix('#')?, 6)
                        || !hex(field(t[18], "want")?.strip_prefix('#')?, 6)
                        || !artifacts(field(t[19], "artifacts")?) { return None; }
                    Some(Record::Frame { check: t[4], matched: false })
                }
                _ => None,
            }
        }
        "TREE" if t.len() == 9 && name(t[4]) => {
            if !name(field(t[5], "window")?) || !hex(field(t[8], "sha256")?, 64) { return None; }
            count(t[6], "commit")?;
            count(t[7], "nodes")?;
            Some(Record::Tree(t[4]))
        }
        "INPUT" if matches!(t.len(), 9 | 10) => {
            for (token, key) in t[4..9].iter().zip(["sent", "processed", "handled", "rejected", "lost"]) {
                count(token, key)?;
            }
            if t.len() == 10 { count(t[9], "missing")?; }
            Some(Record::Input)
        }
        "COLLECT" if t.len() == 9 => {
            if !matches!(field(t[4], "trigger")?, "fail" | "crash" | "timeout" | "save-all") { return None; }
            let screen = field(t[5], "screen")?;
            if !matches!(screen, "at-failure" | "at-collection") && !screen.strip_prefix("none:").is_some_and(name) { return None; }
            count(t[6], "windows")?;
            count(t[7], "remnants")?;
            if !artifacts(field(t[8], "artifacts")?) { return None; }
            Some(Record::Collect)
        }
        _ => None,
    }
}

/// One case's bounded decoder and verdict accounting. `checks` declares FRAME
/// obligations; a TREE names one of those checks but cannot satisfy its FRAME.
pub struct Accounting<'a> {
    suite: &'a str,
    case: &'a str,
    checks: &'a [&'a str],
    collects: bool,
    matched: Vec<bool>,
    seen: Vec<String>,
    records: usize,
    error: Option<String>,
    partial: Vec<u8>,
    discarding: bool,
}

impl<'a> Accounting<'a> {
    pub fn new(suite: &'a str, case: &'a str, checks: &'a [&'a str], collects: bool) -> Result<Self, String> {
        if !name(suite) || !case_name(case) { return Err("invalid result suite or case name".into()); }
        if checks.len() > RECORDS_PER_CASE { return Err("more than 256 declared checks".into()); }
        for (i, check) in checks.iter().enumerate() {
            if !name(check) || checks[..i].contains(check) { return Err("invalid or duplicate declared check".into()); }
        }
        Ok(Self { suite, case, checks, collects, matched: vec![false; checks.len()], seen: Vec::new(),
            records: 0, error: None, partial: Vec::new(), discarding: false })
    }

    /// Remember a transport error without losing an earlier primary record error.
    pub fn refuse(&mut self, reason: &str) {
        if self.error.is_none() { self.error = Some(reason.into()); }
    }

    pub fn accept(&mut self, line: &str) -> Result<(), String> {
        let result = self.accept_record(line);
        if let Err(reason) = &result { self.refuse(reason); }
        result
    }

    fn accept_record(&mut self, line: &str) -> Result<(), String> {
        self.records = self.records.saturating_add(1);
        if self.records > RECORDS_PER_CASE { return Err("result record limit exceeded (256)".into()); }
        let record = parse(line, self.suite, self.case).ok_or("malformed or other-case result record")?;
        let key = match record {
            Record::Frame { check, .. } | Record::Tree(check) => {
                if !self.checks.contains(&check) { return Err(std::format!("undeclared result check `{check}`")); }
                std::format!("{}:{check}", if matches!(record, Record::Tree(_)) { "TREE" } else { "FRAME" })
            }
            Record::Input => "INPUT".into(),
            Record::Collect if self.collects => "COLLECT".into(),
            Record::Collect => return Err("collection was not requested".into()),
        };
        if self.seen.contains(&key) { return Err(std::format!("duplicate result record `{key}`")); }
        self.seen.push(key);
        if let Record::Frame { check, matched } = record {
            let index = self.checks.iter().position(|&name| name == check).unwrap();
            self.matched[index] = matched;
            if !matched { self.refuse(&std::format!("frame check `{check}` mismatched")); }
        }
        Ok(())
    }

    /// Bytes from each read, including the final read after waitpid. Emit only a
    /// complete, valid and bound record; the CASE line reports malformed input.
    pub fn feed(&mut self, bytes: &[u8], mut emit: impl FnMut(&str)) {
        for &byte in bytes {
            if byte == b'\n' {
                if !self.discarding {
                    let partial = std::mem::take(&mut self.partial);
                    match std::str::from_utf8(&partial) {
                        Ok(line) if self.accept(line).is_ok() => emit(line),
                        Ok(_) => {},
                        Err(_) => self.refuse("malformed result record: invalid UTF-8"),
                    }
                }
                self.partial.clear();
                self.discarding = false;
            } else if !self.discarding {
                if self.partial.len() == MAX_RECORD_BYTES - 1 {
                    self.refuse("malformed result record: exceeds 4096 bytes");
                    self.partial.clear();
                    self.discarding = true;
                } else { self.partial.push(byte); }
            }
        }
    }

    pub fn eof(&mut self) {
        if !self.partial.is_empty() || self.discarding { self.refuse("malformed result record: truncated at EOF"); }
    }

    pub fn problem(&self) -> Option<String> {
        self.error.clone().or_else(|| self.checks.iter().zip(&self.matched)
            .find(|(_, matched)| !**matched).map(|(check, _)| std::format!("frame check `{check}` never ran")))
    }
}
