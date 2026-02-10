//! BTRT binary blob parser.
//!
//! Parses the raw memory dump from QMP `pmemsave` and produces
//! a human-readable summary table, KTAP output, and exit code.

use crate::btrt_catalog;
use anyhow::{bail, Context, Result};
use std::path::Path;

/// BTRT magic value (must match kernel).
const BTRT_MAGIC: u64 = 0x4254_5254_0001_0001;

/// Maximum number of test slots.
const MAX_TESTS: usize = 512;

/// Status values (must match kernel BtrtStatus enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BtrtStatus {
    Pending = 0,
    Running = 1,
    Pass = 2,
    Fail = 3,
    Skip = 4,
    Timeout = 5,
}

impl BtrtStatus {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Pending,
            1 => Self::Running,
            2 => Self::Pass,
            3 => Self::Fail,
            4 => Self::Skip,
            5 => Self::Timeout,
            _ => Self::Pending,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Running => "RUNNING",
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Skip => "SKIP",
            Self::Timeout => "TIMEOUT",
        }
    }
}

/// Error code values (must match kernel BtrtErrorCode enum).
fn error_code_name(code: u8) -> &'static str {
    match code {
        0 => "OK",
        1 => "PANIC",
        2 => "ASSERT",
        3 => "TIMEOUT",
        4 => "NOT_FOUND",
        5 => "IO_ERROR",
        6 => "PERMISSION",
        7 => "NO_MEMORY",
        8 => "NO_EXEC",
        9 => "SIGNAL",
        10 => "DEADLOCK",
        11 => "CORRUPT",
        0xFF => "UNKNOWN",
        _ => "?",
    }
}

/// Parsed BTRT header.
#[derive(Debug)]
pub struct BtrtHeader {
    _magic: u64,
    pub total_tests: u32,
    pub tests_completed: u32,
    pub tests_passed: u32,
    pub tests_failed: u32,
    pub boot_start_ns: u64,
    pub boot_end_ns: u64,
}

/// Parsed BTRT entry.
#[derive(Debug)]
pub struct BtrtEntry {
    pub test_id: u16,
    pub status: BtrtStatus,
    pub error_code: u8,
    pub duration_us: u32,
    pub error_detail: u32,
}

/// Parsed BTRT results.
pub struct BtrtResults {
    pub header: BtrtHeader,
    pub entries: Vec<BtrtEntry>,
}

/// Parse a BTRT binary blob from a file.
pub fn parse_file<P: AsRef<Path>>(path: P) -> Result<BtrtResults> {
    let data = std::fs::read(path.as_ref())
        .with_context(|| format!("Failed to read BTRT file: {}", path.as_ref().display()))?;
    parse_blob(&data)
}

/// Parse a BTRT binary blob from raw bytes.
pub fn parse_blob(data: &[u8]) -> Result<BtrtResults> {
    // Header is 48 bytes
    if data.len() < 48 {
        bail!("BTRT data too small: {} bytes (need at least 48)", data.len());
    }

    let magic = u64::from_le_bytes(data[0..8].try_into().unwrap());
    if magic != BTRT_MAGIC {
        bail!(
            "Invalid BTRT magic: {:#018x} (expected {:#018x})",
            magic,
            BTRT_MAGIC
        );
    }

    let header = BtrtHeader {
        _magic: magic,
        total_tests: u32::from_le_bytes(data[8..12].try_into().unwrap()),
        tests_completed: u32::from_le_bytes(data[12..16].try_into().unwrap()),
        tests_passed: u32::from_le_bytes(data[16..20].try_into().unwrap()),
        tests_failed: u32::from_le_bytes(data[20..24].try_into().unwrap()),
        boot_start_ns: u64::from_le_bytes(data[24..32].try_into().unwrap()),
        boot_end_ns: u64::from_le_bytes(data[32..40].try_into().unwrap()),
    };

    // Parse entries (16 bytes each, starting at offset 48)
    let entry_offset = 48;
    let entry_size = 16;
    let max_entries = std::cmp::min(
        MAX_TESTS,
        (data.len() - entry_offset) / entry_size,
    );

    let mut entries = Vec::new();
    for i in 0..max_entries {
        let base = entry_offset + i * entry_size;
        if base + entry_size > data.len() {
            break;
        }
        let test_id = u16::from_le_bytes(data[base..base + 2].try_into().unwrap());
        let status = BtrtStatus::from_u8(data[base + 2]);
        let error_code = data[base + 3];
        let duration_us = u32::from_le_bytes(data[base + 4..base + 8].try_into().unwrap());
        let error_detail = u32::from_le_bytes(data[base + 8..base + 12].try_into().unwrap());

        // Only include non-pending entries (test_id=0 and status=Pending is unused slot)
        if status != BtrtStatus::Pending || test_id != 0 {
            entries.push(BtrtEntry {
                test_id,
                status,
                error_code,
                duration_us,
                error_detail,
            });
        }
    }

    Ok(BtrtResults { header, entries })
}

/// Print a human-readable summary table.
pub fn print_summary(results: &BtrtResults) {
    println!();
    println!("╔══════════════════════════════════════════════════════════════════╗");
    println!("║                   Boot Test Result Table (BTRT)                 ║");
    println!("╠══════════════════════════════════════════════════════════════════╣");
    println!(
        "║  Total: {:3}  Completed: {:3}  Passed: {:3}  Failed: {:3}          ║",
        results.header.total_tests,
        results.header.tests_completed,
        results.header.tests_passed,
        results.header.tests_failed
    );
    println!("╠══════╦════════════════════════════════╦══════════╦══════════════╣");
    println!("║  ID  ║  Test Name                     ║  Status  ║  Details     ║");
    println!("╠══════╬════════════════════════════════╬══════════╬══════════════╣");

    for entry in &results.entries {
        let name = btrt_catalog::test_name(entry.test_id);
        let status_label = entry.status.label();
        let details = if entry.status == BtrtStatus::Fail || entry.status == BtrtStatus::Timeout {
            format!("err={}", error_code_name(entry.error_code))
        } else {
            String::new()
        };
        println!(
            "║ {:>4} ║ {:<30} ║ {:<8} ║ {:<12} ║",
            entry.test_id, name, status_label, details
        );
    }

    println!("╚══════╩════════════════════════════════╩══════════╩══════════════╝");

    let boot_ticks = results
        .header
        .boot_end_ns
        .saturating_sub(results.header.boot_start_ns);
    println!();
    println!(
        "Boot duration: {} ticks ({} → {})",
        boot_ticks, results.header.boot_start_ns, results.header.boot_end_ns
    );
    println!();

    if results.header.tests_failed > 0 {
        println!("RESULT: FAIL ({} tests failed)", results.header.tests_failed);
    } else if results.header.tests_completed == 0 {
        println!("RESULT: NO TESTS COMPLETED");
    } else {
        println!(
            "RESULT: PASS ({}/{} tests passed)",
            results.header.tests_passed, results.header.tests_completed
        );
    }
}

/// Print KTAP-formatted output.
pub fn print_ktap(results: &BtrtResults) {
    println!("KTAP version 1");
    println!("1..{}", results.header.total_tests);

    for entry in &results.entries {
        let name = btrt_catalog::test_name(entry.test_id);
        let seq = entry.test_id + 1;
        match entry.status {
            BtrtStatus::Pass => println!("ok {} {}", seq, name),
            BtrtStatus::Fail => {
                println!(
                    "not ok {} {} # FAIL error_code={} detail={:#x}",
                    seq, name, entry.error_code, entry.error_detail
                );
            }
            BtrtStatus::Skip => println!("ok {} {} # SKIP", seq, name),
            BtrtStatus::Timeout => println!("not ok {} {} # TIMEOUT", seq, name),
            _ => {}
        }
    }

    println!(
        "# {} passed, {} failed, {} skipped",
        results.header.tests_passed,
        results.header.tests_failed,
        results
            .header
            .total_tests
            .saturating_sub(results.header.tests_completed)
    );
}

/// Returns true if all completed tests passed.
pub fn all_passed(results: &BtrtResults) -> bool {
    results.header.tests_failed == 0 && results.header.tests_completed > 0
}

/// Categorize an entry by its test_id range.
fn category_label(test_id: u16) -> &'static str {
    match test_id {
        0..=29 => "Kernel Init",
        30..=49 => "ARM64 Init",
        100..=199 => "Subsystems",
        200..=299 => "Userspace Exec",
        300..=499 => "Userspace Tests",
        _ => "Other",
    }
}

/// Print a detailed table with duration, error details, and category grouping.
pub fn print_detailed(results: &BtrtResults) {
    println!();
    println!("Boot Test Result Table (BTRT) -- Detailed View");
    println!("================================================");
    println!(
        "Total: {}  Completed: {}  Passed: {}  Failed: {}",
        results.header.total_tests,
        results.header.tests_completed,
        results.header.tests_passed,
        results.header.tests_failed
    );

    let boot_ticks = results
        .header
        .boot_end_ns
        .saturating_sub(results.header.boot_start_ns);
    println!("Boot duration: {} ticks\n", boot_ticks);

    // Group entries by category
    let mut current_category = "";

    // Sort entries by test_id for display
    let mut sorted: Vec<&BtrtEntry> = results.entries.iter().collect();
    sorted.sort_by_key(|e| e.test_id);

    println!(
        "{:>5}  {:<32}  {:<8}  {:>10}  {}",
        "ID", "Name", "Status", "Duration", "Details"
    );
    println!("{}", "-".repeat(80));

    for entry in &sorted {
        let cat = category_label(entry.test_id);
        if cat != current_category {
            println!("\n  [{}]", cat);
            current_category = cat;
        }

        let name = btrt_catalog::test_name(entry.test_id);
        let status_label = entry.status.label();

        let duration = if entry.duration_us > 0 {
            format!("{} us", entry.duration_us)
        } else {
            String::new()
        };

        let details = match entry.status {
            BtrtStatus::Fail => format!(
                "err={} detail={:#x}",
                error_code_name(entry.error_code),
                entry.error_detail
            ),
            BtrtStatus::Timeout => "TIMEOUT".to_string(),
            BtrtStatus::Running => "STILL RUNNING".to_string(),
            _ => String::new(),
        };

        println!(
            "{:>5}  {:<32}  {:<8}  {:>10}  {}",
            entry.test_id, name, status_label, duration, details
        );
    }

    println!();
    if results.header.tests_failed > 0 {
        println!("RESULT: FAIL ({} tests failed)", results.header.tests_failed);
    } else if results.header.tests_completed == 0 {
        println!("RESULT: NO TESTS COMPLETED");
    } else {
        println!(
            "RESULT: PASS ({}/{} tests passed)",
            results.header.tests_passed, results.header.tests_completed
        );
    }
}

/// Print results as JSON.
pub fn print_json(results: &BtrtResults) {
    // Manual JSON output to avoid adding serde dependency to xtask
    println!("{{");
    println!("  \"header\": {{");
    println!("    \"total_tests\": {},", results.header.total_tests);
    println!("    \"tests_completed\": {},", results.header.tests_completed);
    println!("    \"tests_passed\": {},", results.header.tests_passed);
    println!("    \"tests_failed\": {},", results.header.tests_failed);
    println!("    \"boot_start_ns\": {},", results.header.boot_start_ns);
    println!("    \"boot_end_ns\": {}", results.header.boot_end_ns);
    println!("  }},");
    println!("  \"entries\": [");

    for (i, entry) in results.entries.iter().enumerate() {
        let name = btrt_catalog::test_name(entry.test_id);
        let trailing = if i + 1 < results.entries.len() { "," } else { "" };
        println!("    {{");
        println!("      \"test_id\": {},", entry.test_id);
        println!("      \"name\": \"{}\",", name);
        println!("      \"category\": \"{}\",", category_label(entry.test_id));
        println!("      \"status\": \"{}\",", entry.status.label());
        println!("      \"error_code\": \"{}\",", error_code_name(entry.error_code));
        println!("      \"error_detail\": {},", entry.error_detail);
        println!("      \"duration_us\": {}", entry.duration_us);
        println!("    }}{}", trailing);
    }

    println!("  ]");
    println!("}}")
}
