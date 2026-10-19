//! A suite's memory record (docs/suites/README.md, "Memory record"). A suite booted as
//! PID 1 for a full run prints, after its START and before its first case, and
//! again once its DONE is out and its processes are reaped:
//!
//! ```text
//! SUITE <id> MEMORY before free=<pages> heap_free_kb=<kB> <owner>=<pages> ... receipts=<n> settle_ms=<ms>
//! SUITE <id> MEMORY after free=<pages> heap_free_kb=<kB> <owner>=<pages> ... receipts=<n> settle_ms=<ms>
//! SUITE <id> MEMORY reclaimed=PASS|FAIL free_before=<pages> free_after=<pages> allowance=<pages>
//! ```
//!
//! - `free` is `/proc/meminfo`'s MemFree in 4 KiB pages, `heap_free_kb` its
//!   KernelHeapFree, and the owners `/proc/breenix/frames`' `owner` rows: the
//!   allocated frames held as user stacks, other user pages, page tables, page
//!   cache, kernel stacks, buffers and other.
//! - Reclamation is deferred: an exited process's tables, and the image an exec
//!   replaced, go back through reclaim receipts, and exited threads' stacks from
//!   an idle CPU. So each sample waits, up to [`SETTLE_MS`], until no receipt is
//!   queued or parked and the free count has held still for [`SETTLE_STEP_MS`];
//!   `receipts` is what was still queued when it stopped waiting, and `settle_ms`
//!   how long it waited.
//! - The suite's memory is reclaimed when `free_after` is at most
//!   [`MEMORY_RECLAIM_ALLOWANCE_PAGES`] below `free_before`. `reclaimed=FAIL`
//!   fails the suite like a failing case (`scripts/suite-verdict.py`).
//! - A record that cannot be read is a `SUITE <id> ERROR memory ...` line, and
//!   `reclaimed=FAIL` with 0 for the side that could not be read.

use std::string::String;
use std::vec::Vec;

use crate::time;

/// How many pages of free memory a suite may leave behind once its processes are
/// reaped. A leak-free suite leaves the runner's own growth, 5 to 7 pages, and the
/// first suite to use a kernel worker that starts on first use (the PTY hangup and
/// TCP timer threads) leaves its 32 stack pages. The largest measured, 37 pages on
/// the R240, with headroom (docs/suites/README.md, "Memory record"). Only the
/// operator loosens it.
pub const MEMORY_RECLAIM_ALLOWANCE_PAGES: u64 = 48;

/// The longest a sample waits for reclamation to finish.
pub const SETTLE_MS: u64 = 5000;
/// How long the free count must hold still, with no receipt queued, for a sample.
pub const SETTLE_STEP_MS: u64 = 100;

/// One reading of the kernel's memory figures.
pub struct Sample {
    pub free: u64,
    heap_free_kb: u64,
    owners: Vec<(String, u64)>,
    receipts: u64,
    settle_ms: u64,
}

/// MemFree in pages and KernelHeapFree in kB from `/proc/meminfo`, and the owner
/// rows and queued receipts from `/proc/breenix/frames`.
fn read() -> Result<Sample, String> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").map_err(|error| std::format!("reading /proc/meminfo failed: {error}"))?;
    let field = |name: &str| meminfo.lines()
        .find_map(|line| line.strip_prefix(name)?.trim().strip_suffix("kB")?.trim().parse::<u64>().ok())
        .ok_or_else(|| std::format!("/proc/meminfo has no {name} line"));
    let free = field("MemFree:")? / 4;
    let heap_free_kb = field("KernelHeapFree:")?;
    let frames = std::fs::read_to_string("/proc/breenix/frames")
        .map_err(|error| std::format!("reading /proc/breenix/frames failed: {error}"))?;
    let mut owners = Vec::new();
    let mut receipts = None;
    for line in frames.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["owner", name, count] => owners.push((String::from(*name), count.parse().map_err(|_| std::format!("bad owner row {line:?}"))?)),
            ["receipts", pending, parked] => {
                let count = |word: &str, key: &str| word.strip_prefix(key).and_then(|n| n.parse::<u64>().ok());
                receipts = count(pending, "pending=").zip(count(parked, "parked=")).map(|(a, b)| a + b);
            }
            _ => {}
        }
    }
    if owners.is_empty() {
        return Err(String::from("/proc/breenix/frames has no owner rows"));
    }
    let receipts = receipts.ok_or("/proc/breenix/frames has no receipts row")?;
    Ok(Sample { free, heap_free_kb, owners, receipts, settle_ms: 0 })
}

/// A reading once reclamation has settled, or after [`SETTLE_MS`] of waiting.
pub fn settled() -> Result<Sample, String> {
    let start = monotonic_ms();
    let mut last = read()?;
    loop {
        let waited = monotonic_ms().saturating_sub(start);
        if waited >= SETTLE_MS {
            last.settle_ms = waited;
            return Ok(last);
        }
        let _ = time::sleep_ms(SETTLE_STEP_MS);
        let now = read()?;
        let still = now.receipts == 0 && last.receipts == 0 && now.free == last.free;
        last = now;
        if still {
            last.settle_ms = monotonic_ms().saturating_sub(start);
            return Ok(last);
        }
    }
}

impl Sample {
    /// `SUITE <id> MEMORY <when> ...`.
    pub fn record(&self, id: &str, when: &str) -> String {
        let mut line = std::format!("SUITE {id} MEMORY {when} free={} heap_free_kb={}", self.free, self.heap_free_kb);
        for (owner, count) in &self.owners {
            line.push_str(&std::format!(" {owner}={count}"));
        }
        line.push_str(&std::format!(" receipts={} settle_ms={}", self.receipts, self.settle_ms));
        line
    }
}

/// The verdict line, and whether the memory was reclaimed. A side that could not
/// be read counts as 0 free pages and the verdict is FAIL.
pub fn verdict(id: &str, before: Option<&Sample>, after: Option<&Sample>) -> (String, bool) {
    let reclaimed = match (before, after) {
        (Some(before), Some(after)) => after.free + MEMORY_RECLAIM_ALLOWANCE_PAGES >= before.free,
        _ => false,
    };
    let line = std::format!(
        "SUITE {id} MEMORY reclaimed={} free_before={} free_after={} allowance={}",
        if reclaimed { "PASS" } else { "FAIL" },
        before.map_or(0, |sample| sample.free),
        after.map_or(0, |sample| sample.free),
        MEMORY_RECLAIM_ALLOWANCE_PAGES,
    );
    (line, reclaimed)
}

fn monotonic_ms() -> u64 {
    time::now_monotonic().map_or(0, |ts| (ts.as_nanos() / 1_000_000) as u64)
}
