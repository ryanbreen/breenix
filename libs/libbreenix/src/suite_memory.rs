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
//! - Boot's own workers are not the suite's: each sample first waits, up to
//!   [`BOOT_SETTLE_MS`], until `/proc/breenix/frames` reports `boot settled=1`
//!   (on bare-metal x86-64, after the boot thread's tail and the tick census).
//! - Reclamation is deferred: an exited process's tables, and the image an exec
//!   replaced, go back through reclaim receipts, and exited threads' stacks from
//!   an idle CPU. So each sample then waits, up to [`SETTLE_MS`], until no receipt is
//!   outstanding (including a receipt owned by the drain) and the free count
//!   has held still for [`SETTLE_STEP_MS`];
//!   `receipts` is what was still queued when it stopped waiting, and `settle_ms`
//!   how long it waited, both waits together. The proc row `reclaim outstanding`
//!   includes receipts not yet published and receipts temporarily removed by a drain.
//! - Live unmap also frees empty page tables. In #2729 the PID-1 runner's malloc
//!   mapped each block; empty tables for visited 2 MiB windows stayed until exit,
//!   retaining 29-35 frames per BreenGL run. The shared unmap path now reclaims
//!   tables no mapping uses on both architectures. `BREENGEL TABLES` at before,
//!   finale, retired and after reports `runner_pte_kb`, `processes`, `all_pte_kb`
//!   and `top_pte` (the largest min(3, processes) pid:name:KiB owners). Failed reads
//!   produce `unavailable`; the GT 710 scorer and ownership-report parser reject
//!   incomplete censuses (G35b). See the memory-record documentation for the format.
//! - The suite's memory is reclaimed when `free_after` is at most
//!   [`MEMORY_RECLAIM_ALLOWANCE_PAGES`] below `free_before`. `reclaimed=FAIL`
//!   fails the suite like a failing case (`scripts/suite-verdict.py`), which also
//!   refuses a record whose allowance differs from this constant (G35(3)).
//! - A record that cannot be read is a `SUITE <id> ERROR memory ...` line, and
//!   `reclaimed=FAIL` with 0 for the side that could not be read.

use std::string::String;
use std::vec::Vec;

use crate::time;

/// How many pages of free memory a suite may leave behind once its processes are
/// reaped. A leak-free suite leaves the runner's own growth: 5 to 7 pages in a
/// sequence, 38 for the memory suite booted alone, whose runner draws its live
/// panel (the 2026-10-19 measurements in docs/suites/README.md, "Memory record").
/// The PTY hangup and TCP timer workers now start at boot (#2233); the allocation
/// cache is released before sampling, and live unmap reclaims unused tables
/// (#2729). These fixes return owners' pages rather than enlarge this allowance.
/// Only the operator loosens it; host scorers read this definition (G35(3)).
pub const MEMORY_RECLAIM_ALLOWANCE_PAGES: u64 = 48;

/// The longest a sample waits for reclamation to finish.
pub const SETTLE_MS: u64 = 5000;
/// How long the free count must hold still, with no receipt outstanding, for a sample.
pub const SETTLE_STEP_MS: u64 = 100;
/// The longest a sample waits first for boot's own work to finish: the kernel's
/// `boot settled=1` row, which on bare-metal x86-64 follows the tick census's
/// per-CPU windows (0.5 s each).
pub const BOOT_SETTLE_MS: u64 = 30_000;

/// Whether `/proc/breenix/frames` says boot's own work is over. A kernel without
/// the row reports no boot work to wait for.
fn boot_settled() -> bool {
    std::fs::read_to_string("/proc/breenix/frames")
        .map_or(true, |text| !text.lines().any(|line| line.trim() == "boot settled=0"))
}

/// The kernel owns the immutable open-time snapshot. Keeping just its fd in
/// userspace adds no observer mappings to the memory sample. Close-on-exec
/// prevents a case's exec from keeping the baseline alive.
struct Ownership(Result<crate::types::Fd, crate::error::Error>);
impl Ownership {
    fn capture() -> Self {
        const O_CLOEXEC: u32 = 0x80000;
        Self(crate::fs::open("/proc/breenix/memory-owners", crate::fs::O_RDONLY | O_CLOEXEC))
    }
    fn detail(&self) -> String {
        let fd = match &self.0 {
            Ok(fd) => *fd,
            Err(error) => return std::format!("snapshot unavailable={error}\n"),
        };
        let mut text = String::new();
        let mut buf = [0u8; 4096];
        loop {
            match crate::io::read(fd, &mut buf) {
                Ok(0) => return text,
                Ok(n) => text.push_str(std::str::from_utf8(&buf[..n]).unwrap_or("snapshot invalid-utf8\n")),
                Err(error) => { text.push_str(&std::format!("snapshot read-error={error}\n")); return text; }
            }
        }
    }
}
impl Drop for Ownership {
    fn drop(&mut self) { if let Ok(fd) = &self.0 { let _ = crate::io::close(*fd); } }
}

/// One reading of the kernel's memory figures.
pub struct Sample {
    pub free: u64,
    ownership: Ownership,
    heap_free_kb: u64,
    owners: Vec<(String, u64)>,
    receipts: u64,
    reclaim_outstanding: u64,
    settle_ms: u64,
}

/// MemFree in pages and KernelHeapFree in kB from `/proc/meminfo`, and the owner
/// rows and queued receipts from `/proc/breenix/frames`.
fn read() -> Result<Sample, String> {
    let ownership = Ownership::capture();
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
    let mut reclaim_outstanding = None;
    for line in frames.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["owner", name, count] => owners.push((String::from(*name), count.parse().map_err(|_| std::format!("bad owner row {line:?}"))?)),
            ["receipts", pending, parked] => {
                let count = |word: &str, key: &str| word.strip_prefix(key).and_then(|n| n.parse::<u64>().ok());
                receipts = count(pending, "pending=").zip(count(parked, "parked=")).map(|(a, b)| a + b);
            }
            ["reclaim", outstanding] => {
                reclaim_outstanding = outstanding.strip_prefix("outstanding=")
                    .and_then(|n| n.parse::<u64>().ok());
            }
            _ => {}
        }
    }
    if owners.is_empty() {
        return Err(String::from("/proc/breenix/frames has no owner rows"));
    }
    let receipts = receipts.ok_or("/proc/breenix/frames has no receipts row")?;
    let reclaim_outstanding = reclaim_outstanding.ok_or("/proc/breenix/frames has no reclaim outstanding row")?;
    Ok(Sample { free, ownership, heap_free_kb, owners, receipts, reclaim_outstanding, settle_ms: 0 })
}

/// Give back the free blocks the C library's allocation cache keeps, so a sample
/// counts the memory the runner uses and not free blocks it holds for reuse (up to
/// 128 pages: 16 one-page blocks in each of 8 size classes).
fn release_allocation_cache() {
    #[cfg(target_os = "breenix")]
    {
        extern "C" {
            fn __breenix_malloc_release_cached() -> usize;
        }
        // SAFETY: the cache holds only freed blocks; no live allocation is touched.
        unsafe { __breenix_malloc_release_cached() };
    }
}

/// A reading once boot's own work is over (or after [`BOOT_SETTLE_MS`]) and
/// reclamation has settled (or after a further [`SETTLE_MS`] of waiting).
/// `settle_ms` counts both waits. Each reading first releases the runner's
/// allocation cache.
pub fn settled() -> Result<Sample, String> {
    let begun = monotonic_ms();
    while !boot_settled() && monotonic_ms().saturating_sub(begun) < BOOT_SETTLE_MS {
        let _ = time::sleep_ms(SETTLE_STEP_MS);
    }
    let start = monotonic_ms();
    release_allocation_cache();
    let mut last = read()?;
    loop {
        let waited = monotonic_ms().saturating_sub(start);
        if waited >= SETTLE_MS {
            last.settle_ms = monotonic_ms().saturating_sub(begun);
            return Ok(last);
        }
        let _ = time::sleep_ms(SETTLE_STEP_MS);
        release_allocation_cache();
        let now = read()?;
        let still = now.receipts == 0 && last.receipts == 0
            && now.reclaim_outstanding == 0 && last.reclaim_outstanding == 0
            && now.free == last.free;
        last = now;
        if still {
            last.settle_ms = monotonic_ms().saturating_sub(begun);
            return Ok(last);
        }
    }
}

impl Sample {
    /// Export the exact snapshot paired with this reading.
    pub fn ownership_detail(&self) -> String { self.ownership.detail() }

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
