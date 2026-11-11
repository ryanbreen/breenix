//! Fixed-capacity numeric restart journal with task-context formatting.
use alloc::{collections::BTreeMap, format, string::String, vec::Vec};
use core::fmt::Write;
use core::sync::atomic::{
    AtomicU64,
    Ordering::{AcqRel, Acquire, Relaxed, Release},
};

// Even versions are published. Producers attempt one CAS per cell and count a
// busy cell as lost: they may have interrupted its owner on the same CPU.
struct Cell<const W: usize> {
    version: AtomicU64,
    words: [AtomicU64; W],
}
impl<const W: usize> Cell<W> {
    const fn new() -> Self {
        Self {
            version: AtomicU64::new(0),
            words: [const { AtomicU64::new(0) }; W],
        }
    }
    fn read(&self) -> Option<[u64; W]> {
        let version = self.version.load(Acquire);
        if version & 1 != 0 {
            return None;
        }
        let words = core::array::from_fn(|i| self.words[i].load(Relaxed));
        core::sync::atomic::fence(Acquire);
        (self.version.load(Relaxed) == version).then_some(words)
    }
    fn update(&self, f: impl FnOnce([u64; W]) -> Option<[u64; W]>) -> bool {
        let version = self.version.load(Acquire);
        if version & 1 != 0
            || self
                .version
                .compare_exchange(version, version.wrapping_add(1), AcqRel, Relaxed)
                .is_err()
        {
            return false;
        }
        let old = core::array::from_fn(|i| self.words[i].load(Relaxed));
        let next = f(old);
        if let Some(words) = next {
            for (dst, src) in self.words.iter().zip(words) {
                dst.store(src, Relaxed);
            }
        }
        self.version.store(version.wrapping_add(2), Release);
        next.is_some()
    }
}

/// Numeric state modes: 0 runnable, 1 dispatched, 2 blocked, 3 ended.
/// Wait kinds 1..5 match WaitSite; 6 timer, 7 signal, 8 generic wait.
pub struct Accounting<const N: usize, const T: usize, const D: usize> {
    // Event: kind, window, time, pid, tid, a, b, c, d, reservation.
    events: [Cell<10>; N],
    next: AtomicU64,
    lost: AtomicU64,
    // Latest thread: tid, pid, since, mode, wait kind, wait object.
    threads: [Cell<6>; T],
    thread_keys: [AtomicU64; T],
    state_lost: AtomicU64,
    // Disk: id,pid,tid,device,op,sector,bytes,enqueue,gate,submit,irq,wake,release,resume.
    disks: [Cell<14>; D],
    disk_next: AtomicU64,
    disk_lost: AtomicU64,
    generation: AtomicU64,
    active: AtomicU64,
    owner: AtomicU64,
    exported: AtomicU64,
}
impl<const N: usize, const T: usize, const D: usize> Accounting<N, T, D> {
    pub const fn new() -> Self {
        Self {
            events: [const { Cell::new() }; N],
            next: AtomicU64::new(0),
            lost: AtomicU64::new(0),
            threads: [const { Cell::new() }; T],
            thread_keys: [const { AtomicU64::new(0) }; T],
            state_lost: AtomicU64::new(0),
            disks: [const { Cell::new() }; D],
            disk_next: AtomicU64::new(0),
            disk_lost: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            active: AtomicU64::new(0),
            owner: AtomicU64::new(0),
            exported: AtomicU64::new(0),
        }
    }
    fn push(&self, mut event: [u64; 10]) {
        let seq = self.next.fetch_add(1, Relaxed) + 1;
        event[9] = seq;
        if !self.events[(seq as usize - 1) % N].update(|old| (old[9] < seq).then_some(event)) {
            self.lost.fetch_add(1, Relaxed);
        }
    }
    fn interval(&self, window: u64, old: [u64; 6], end: u64) {
        if window != 0 && old[0] != 0 && old[3] != 3 && old[2] <= end {
            self.push([
                3, window, end, old[1], old[0], old[2], old[3], old[4], old[5], 0,
            ]);
        }
    }
    /// Called at state publication, including initial enqueue and final retirement.
    pub fn state(&self, ns: u64, pid: u64, tid: u64, mode: u64, kind: u64, object: u64) {
        if tid == 0 {
            return;
        }
        let existing = (0..T)
            .map(|probe| (tid as usize + probe) % T)
            .find(|&slot| self.thread_keys[slot].load(Acquire) == tid);
        let slot = existing.or_else(|| {
            (0..T)
                .map(|probe| (tid as usize + probe) % T)
                .find(|&slot| {
                    let key = self.thread_keys[slot].load(Acquire);
                    let reusable = key == 0 || self.threads[slot].read().is_some_and(|r| r[3] == 3);
                    reusable
                        && self.thread_keys[slot]
                            .compare_exchange(key, tid, AcqRel, Acquire)
                            .is_ok()
                })
        });
        if let Some(slot) = slot {
            let stored = self.threads[slot].update(|old| {
                if old[0] == tid && old[1] == pid && old[3..] == [mode, kind, object] {
                    return Some(old);
                }
                if old[0] == tid {
                    self.interval(self.active.load(Acquire), old, ns);
                }
                Some([tid, pid, ns, mode, kind, object])
            });
            if stored {
                return;
            }
        }
        self.state_lost.fetch_add(1, Relaxed);
    }
    pub fn begin(&self, ns: u64, pid: u64, vcpus: u64) {
        // A replacement before its predecessor composed keeps a truncated window.
        let previous = self.owner.load(Acquire);
        if self.active.load(Acquire) != 0 {
            self.finish(ns, previous, false);
        }
        let window = self.generation.fetch_add(1, Relaxed) + 1;
        self.owner.store(pid, Release);
        self.push([1, window, ns, pid, 0, vcpus, 0, 0, 0, 0]);
        self.active.store(window, Release);
    }
    fn finish(&self, ns: u64, pid: u64, complete: bool) {
        let window = self.active.load(Acquire);
        if window == 0 || self.owner.load(Acquire) != pid {
            return;
        }
        // Keep producers active until open intervals have been copied. A concurrent
        // transition can overlap a copy; the exporter unions those intervals.
        for cell in &self.threads {
            match cell.read() {
                Some(row) => self.interval(window, row, ns),
                None => {
                    self.state_lost.fetch_add(1, Relaxed);
                }
            }
        }
        self.push([2, window, ns, pid, 0, u64::from(complete), 0, 0, 0, 0]);
        self.active
            .compare_exchange(window, 0, AcqRel, Acquire)
            .ok();
    }
    pub fn end(&self, ns: u64, pid: u64) {
        self.finish(ns, pid, true);
    }
    pub fn active_window(&self) -> u64 {
        self.active.load(Acquire)
    }
    pub fn is_active(&self) -> bool {
        self.active_window() != 0
    }
    pub fn identity(&self, ns: u64, pid: u64, tid: u64, group: u64, name: &str) {
        let window = self.active_window();
        if window == 0 {
            return;
        }
        let mut bytes = [0u8; 24];
        let len = name.len().min(bytes.len());
        bytes[..len].copy_from_slice(&name.as_bytes()[..len]);
        let a = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        let b = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let c = u64::from_le_bytes(bytes[16..].try_into().unwrap());
        self.push([6, window, ns, pid, tid, group, a, b, c, 0]);
    }
    pub fn queue(&self, ns: u64, cpu: u64, len: u64) {
        let window = self.active.load(Acquire);
        if window != 0 {
            self.push([4, window, ns, 0, 0, cpu, len, 0, 0, 0]);
        }
    }
    /// File-call records include competing submitters and clone owner PIDs,
    /// using raw ABI numbers.
    pub fn syscall(&self, ns: u64, pid: u64, tid: u64, nr: u64, entry: bool, result: i64) {
        let window = self.active.load(Acquire);
        if window != 0 {
            self.push([
                5,
                window,
                ns,
                pid,
                tid,
                nr,
                u64::from(entry),
                result as u64,
                0,
                0,
            ]);
        }
    }
    pub fn disk_enqueue(
        &self,
        ns: u64,
        pid: u64,
        tid: u64,
        device: u64,
        op: u64,
        sector: u64,
        bytes: u64,
    ) -> u64 {
        let id = self.disk_next.fetch_add(1, Relaxed) + 1;
        let row = [
            id, pid, tid, device, op, sector, bytes, ns, 0, 0, 0, 0, 0, 0,
        ];
        if !self.disks[(id as usize - 1) % D].update(|old| (old[0] < id).then_some(row)) {
            self.disk_lost.fetch_add(1, Relaxed);
        }
        id
    }
    /// Stage 1 gate acquire, 2 device publication, 3 completion IRQ, 4 waiter
    /// made runnable, 5 gate release, 6 wait return. Absent stages retain the initial 0 stamp.
    pub fn disk_stage(&self, id: u64, stage: usize, ns: u64) {
        if id == 0 || !(1..=6).contains(&stage) {
            return;
        }
        if !self.disks[(id as usize - 1) % D].update(|mut row| {
            if row[0] != id {
                return None;
            }
            if stage == 4 && row[10] == 0 {
                return Some(row);
            }
            if row[7 + stage] == 0 {
                row[7 + stage] = ns;
            }
            Some(row)
        }) {
            self.disk_lost.fetch_add(1, Relaxed);
        }
    }
    /// Task-context export. Completed windows are emitted once; open windows
    /// remain exportable after a watchdog dump. Storage is not cleared by reads.
    pub fn text(&self, now: u64) -> String {
        let count = self.next.load(Acquire);
        let first = count.saturating_sub(N as u64) + 1;
        let mut rows: Vec<_> = self
            .events
            .iter()
            .filter_map(Cell::read)
            .filter(|r| r[9] >= first && r[9] <= count)
            .collect();
        let unavailable = count.min(N as u64).saturating_sub(rows.len() as u64);
        let mut state_unavailable = 0u64;
        let window = self.active.load(Acquire);
        if window != 0 {
            for cell in &self.threads {
                if let Some(r) = cell.read() {
                    if r[0] != 0 && r[3] != 3 && r[2] <= now {
                        rows.push([3, window, now, r[1], r[0], r[2], r[3], r[4], r[5], 0]);
                    }
                } else {
                    state_unavailable += 1;
                }
            }
        }
        let after = self.exported.load(Acquire);
        let mut windows: Vec<_> = rows
            .iter()
            .filter(|r| r[0] == 1 && r[1] > after)
            .copied()
            .collect();
        windows.sort_unstable_by_key(|r| r[1]);
        let disk_count = self.disk_next.load(Acquire);
        let mut out=format!("account-buffer events={} overwritten={} lost={} state_lost={} disk_overwritten={} disk_lost={} unavailable={unavailable} state_unavailable={state_unavailable}\n",
            count.min(N as u64),count.saturating_sub(N as u64),self.lost.load(Relaxed),self.state_lost.load(Relaxed),disk_count.saturating_sub(D as u64),self.disk_lost.load(Relaxed));
        for start in windows {
            let id = start[1];
            let begin = start[2];
            let ended = rows.iter().find(|r| r[0] == 2 && r[1] == id);
            let end = ended.map_or(now, |r| r[2]);
            let complete = ended.map_or(0, |r| r[5]);
            let _=writeln!(out,"account-window id={id} pid={} begin_ns={begin} end_ns={end} complete={complete} vcpus={}",start[3],start[5]);
            let mut intervals: Vec<_> = rows
                .iter()
                .filter(|r| r[0] == 3 && r[1] == id)
                .copied()
                .collect();
            intervals.sort_unstable_by_key(|r| (r[4], r[5], r[2]));
            let mut totals = BTreeMap::<(u64, u64), [u64; 3]>::new();
            let mut waits = BTreeMap::<(u64, u64, u64, u64), u64>::new();
            let mut covered = BTreeMap::<u64, u64>::new();
            for r in intervals {
                let from = r[5].max(begin).max(*covered.get(&r[4]).unwrap_or(&begin));
                let to = r[2].min(end);
                if to <= from || r[6] > 2 {
                    continue;
                }
                let duration = to - from;
                let total = totals.entry((r[3], r[4])).or_default();
                total[r[6] as usize] += duration;
                if r[6] == 2 {
                    *waits.entry((r[3], r[4], r[7], r[8])).or_default() += duration;
                }
                covered.insert(r[4], to);
            }
            for ((pid, tid), t) in totals {
                let _=writeln!(out,"account-thread window={id} pid={pid} tid={tid} on_cpu_ns={} runnable_ns={} blocked_ns={}",t[1],t[0],t[2]);
            }
            for ((pid, tid, kind, object), duration) in waits {
                let _=writeln!(out,"account-wait window={id} pid={pid} tid={tid} kind={kind} object={object} blocked_ns={duration}");
            }
            for r in rows
                .iter()
                .filter(|r| r[1] == id && r[2] >= begin && r[2] <= end)
            {
                if out.len() >= 8 * 1024 * 1024 {
                    out.push_str("account-export-truncated limit_bytes=8388608\n");
                    return out;
                }
                if r[0] == 6 {
                    let mut bytes = [0u8; 24];
                    for (word, part) in r[6..9].iter().zip(bytes.chunks_mut(8)) {
                        part.copy_from_slice(&word.to_le_bytes());
                    }
                    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                    let name =
                        String::from_utf8_lossy(&bytes[..len]).replace(char::is_whitespace, "_");
                    let _ = writeln!(
                        out,
                        "account-identity window={id} pid={} tid={} tgid={} name={name}",
                        r[3], r[4], r[5]
                    );
                }
                if r[0] == 4 {
                    let _ = writeln!(
                        out,
                        "account-runqueue window={id} ns={} cpu={} len={}",
                        r[2], r[5], r[6]
                    );
                }
                if r[0] == 5 {
                    let _ = writeln!(
                        out,
                        "file-syscall window={id} pid={} tid={} nr={} entry={} ns={} result={}",
                        r[3], r[4], r[5], r[6], r[2], r[7] as i64
                    );
                }
            }
            let mut disks: Vec<_> = self
                .disks
                .iter()
                .filter_map(Cell::read)
                .filter(|r| r[0] != 0 && r[7] <= end && (r[12] == 0 || r[12] >= begin))
                .collect();
            disks.sort_unstable_by_key(|r| r[0]);
            for r in disks {
                if out.len() >= 8 * 1024 * 1024 {
                    out.push_str("account-export-truncated limit_bytes=8388608\n");
                    return out;
                }
                let _=writeln!(out,"disk-request id={} pid={} tid={} device={} op={} sector={} bytes={} enqueue_ns={} gate_ns={} submit_ns={} irq_ns={} wake_ns={} release_ns={} resume_ns={} window={id}",r[0],r[1],r[2],r[3],r[4],r[5],r[6],r[7],r[8],r[9],r[10],r[11],r[12],r[13]);
            }
            if ended.is_some() && unavailable == 0 && state_unavailable == 0 {
                self.exported.fetch_max(id, Release);
            }
        }
        out
    }
}

#[cfg(not(test))]
#[path = "accounting_hooks.rs"]
mod hooks;
#[cfg(not(test))]
pub use hooks::*;
