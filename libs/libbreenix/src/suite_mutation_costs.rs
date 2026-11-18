//! Read-only kernel-counter transport for 520(s)'s failure-only serial path.
//! The root mount includes work by the case, its supervisor and other tasks.
//! Snapshot deltas bracket a case, not a process or a causal UI attribution.
//! Completed durations may include a span begun before the first snapshot;
//! timestamps and active counts expose those boundary-crossing operations.
//! max_ns is the mount-lifetime high-water mark of completed spans (583(x10)),
//! carried unchanged rather than subtracted across the case.
//! A missing baseline disables these optional observations (e.g. host probes).
use std::io::Read;
const LIMIT: usize = 64;
const BYTES: u64 = 32 * 1024;
const OPS: &[&str] = &[
    "create",
    "link",
    "unlink",
    "rmdir",
    "rename",
    "setattr",
    "write",
    "resize",
    "writeback",
    "fsync",
    "commit",
    "xattr",
    "foreground",
];
const PHASES: &[&str] = &[
    "operation",
    "reservation",
    "inode-edit",
    "publication",
    "wait",
    "prepare-files",
    "next-files-view",
    "write-cached",
    "device-flush",
    "mount-exclusion",
    "retain-open",
    "apply-touches",
    "drain-dirty",
    "prepare-reap",
    "cleanup",
    "completion",
    "mount-excluded",
];
#[derive(Clone, Copy, Default)]
struct Row {
    mount: u64,
    root: u64,
    op: usize,
    phase: usize,
    started: u64,
    completed: u64,
    ns: u64,
    last_start: u64,
    last_end: u64,
    max_ns: u64,
}
pub(super) struct Snapshot {
    rows: Vec<Row>,
    available: bool,
}
impl Snapshot {
    pub fn read() -> Option<Self> {
        let mut bytes = String::new();
        std::fs::File::open("/proc/breenix/fs-mutation-costs")
            .ok()?
            .take(BYTES)
            .read_to_string(&mut bytes)
            .ok()?;
        Some(Self::parse(&bytes))
    }
    pub fn parse(text: &str) -> Self {
        let mut rows = Vec::new();
        let mut root = false;
        for line in text.lines().take(256) {
            if !line.starts_with("mutation_cost ") {
                if line.contains(" breenfs ") || line.contains(" durable_generation=") {
                    root = line.starts_with("root breenfs ") || line.starts_with("/ breenfs ");
                }
                continue;
            }
            if !root || rows.len() == LIMIT {
                continue;
            }
            let fields: Vec<_> = line.split_whitespace().take(12).collect();
            if !matches!(fields.len(), 10 | 11) {
                continue;
            }
            let field = |i: usize, key: &str| fields[i].strip_prefix(key);
            let number = |i, key| field(i, key)?.parse::<u64>().ok();
            let parsed = (|| {
                Some(Row {
                    mount: number(1, "mount=")?,
                    root: number(2, "root=")?,
                    op: OPS.iter().position(|&s| Some(s) == field(3, "op="))?,
                    phase: PHASES.iter().position(|&s| Some(s) == field(4, "phase="))?,
                    started: number(5, "started=")?,
                    completed: number(6, "completed=")?,
                    ns: number(7, "total_ns=")?,
                    last_start: number(8, "last_start_ns=")?,
                    last_end: number(9, "last_end_ns=")?,
                    max_ns: if fields.len() == 11 { number(10, "max_ns=")? } else { 0 },
                })
            })();
            if let Some(row) = parsed {
                if !rows.iter().any(|r: &Row| r.key() == row.key()) {
                    rows.push(row);
                }
            }
        }
        Self {
            rows,
            available: true,
        }
    }
    /// Failure gating precedes the reader; output stays in the serial observation path.
    pub fn records(
        &self,
        suite: &str,
        case: &str,
        failed: bool,
        read: impl FnOnce() -> Option<Self>,
    ) -> Vec<String> {
        if !failed || !self.available {
            return Vec::new();
        }
        let Some(after) = read() else {
            return vec![format!(
                "CASE_OBSERVATION {suite} {case} op=vfs-mutation unavailable=read scope=mount"
            )];
        };
        after.rows.iter().filter_map(|row| {
            let old = self.rows.iter().find(|r| r.key() == row.key()).copied().unwrap_or_default();
            let started = row.started.saturating_sub(old.started);
            let completed = row.completed.saturating_sub(old.completed);
            let ns = row.ns.saturating_sub(old.ns);
            if started == 0 && completed == 0 && ns == 0 { return None; }
            Some(format!("CASE_OBSERVATION {suite} {case} op=vfs-{}-{} started={started} completed={completed} total_ns={ns} active={} last_start_ns={} last_end_ns={} mount={} root={} max_ns={} scope=mount",
                OPS[row.op], PHASES[row.phase], row.started.saturating_sub(row.completed),
                row.last_start, row.last_end, row.mount, row.root, row.max_ns))
        }).collect()
    }
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            available: false,
        }
    }
}
impl Row {
    fn key(&self) -> (u64, u64, usize, usize) {
        (self.mount, self.root, self.op, self.phase)
    }
}
