//! cowfs-check, written from docs/fs/cowfs-format.md alone. It shares only the CRC-32C and
//! SipHash primitives with the writer.
//!
//! Order: the eight slots (S1, S2), recovery's choice of M (S3), the walk of M's trees
//! (N1-N6, C1's CRC counts), S4, in full mode the data CRCs (C1), the space accounting
//! (A1-A6), then each root's filesystem tree (F1-F10, with F7's zero tails in full mode).
//! claim-lint:ok: docs/fs/cowfs-format.md sections 3 and 5.

mod data;
mod files;
mod image;
mod node;
mod raw;
mod recovery;
mod report;
mod slots;
mod space;
mod walk;

use std::io;
use std::path::Path;

use report::Report;
use slots::Slot;
use walk::Walk;

/// Exit 8: the file is unreadable or no slot is valid. It takes precedence over 0, 1 and 4.
pub const EXIT_UNREADABLE: i32 = 8;

pub struct Outcome {
    /// Findings, one stdout line each.
    pub lines: Vec<String>,
    pub code: i32,
    /// Why the image could not be checked, for stderr.
    pub fatal: Option<String>,
}

pub fn check(path: &Path, full: bool) -> Outcome {
    let mut report = Report::new();
    let result = run(path, full, &mut report);
    let (code, fatal) = match result {
        Ok(true) => (report.exit_code(), None),
        Ok(false) => (EXIT_UNREADABLE, Some("no valid superblock slot".to_string())),
        Err(e) => (EXIT_UNREADABLE, Some(format!("{}: {e}", path.display()))),
    };
    Outcome { lines: report.lines, code, fatal }
}

/// Ok(false) when no slot is valid.
fn run(path: &Path, full: bool, report: &mut Report) -> io::Result<bool> {
    let image = image::Image::open(path)?;
    let slots = slots::read(&image, report)?;
    if slots.is_empty() {
        return Ok(false);
    }
    slots::check_agreement(&slots, report);
    let m = match recovery::choose(&image, &slots, report)? {
        Some(m) => m,
        None => return Ok(true),
    };
    let walk = walk::run(&image, m, report)?;
    check_totals(m, &walk, report);
    if full {
        data::verify(&image, &walk, report)?;
    }
    for (id, detail) in walk.space_findings() {
        report.error(id, detail);
    }
    walk.check_files(full, report)?;
    Ok(true)
}

/// S4: free_blocks, next_root_id and global_reserve in M's superblock.
fn check_totals(m: &Slot, walk: &Walk<'_>, report: &mut Report) {
    let name = m.name();
    match walk.free_total {
        Some(free) if free == m.free_blocks => {}
        Some(free) => report.error("S4", format!("{name}: free_blocks {}, FREE items sum to {free}", m.free_blocks)),
        None => report.error("S4", format!("{name}: FREE item lengths overflow 64 bits")),
    }
    if let Some(root) = walk.roots.iter().find(|r| r.id >= m.next_root_id) {
        report.error("S4", format!("{name}: next_root_id {} does not exceed root id {}", m.next_root_id, root.id));
    }
    let reserve = (m.total_blocks / 200).clamp(4096, 262_144);
    if m.global_reserve != reserve {
        report.error("S4", format!("{name}: global_reserve {}, section 2.8 gives {reserve}", m.global_reserve));
    }
}
