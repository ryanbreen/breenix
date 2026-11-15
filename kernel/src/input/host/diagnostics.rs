//! #2572: always-on progress, frozen on the first serial RX overflow.
//! IRQ recording never emits output. The independent census task emits once.
pub use super::progress::{Count, Site};
use super::progress::{Lane, Progress};
use core::sync::atomic::AtomicU64;

pub static PROGRESS: Progress = Progress::new();
pub static COMMAND: Lane = Lane::new();
pub static INPUT: Lane = Lane::new();
pub static RENDER: Lane = Lane::new();
pub static INPUT_TID: AtomicU64 = AtomicU64::new(0);
pub static RENDER_TID: AtomicU64 = AtomicU64::new(0);
pub static LAST_ACCEPT: AtomicU64 = AtomicU64::new(0);
pub static LAST_ACK: AtomicU64 = AtomicU64::new(0);

/// Shared VT helpers also run in syscalls. Only the renderer may update its
/// lane, so another thread cannot overwrite the suspended renderer's site.
pub fn render_site(site: Site) -> Option<super::progress::At<'static>> {
    #[cfg(target_arch = "x86_64")]
    if crate::per_cpu::current_thread_id_lock_free()
        == Some(RENDER_TID.load(core::sync::atomic::Ordering::Relaxed))
    {
        return Some(RENDER.enter(site));
    }
    #[cfg(target_arch = "aarch64")]
    let _ = site; // The overflow witness and renderer TID are x86-specific.
    None
}

pub struct Returned(Count);
pub fn call(enter: Count, exit: Count) -> Returned {
    PROGRESS.note(enter);
    Returned(exit)
}
impl Drop for Returned {
    fn drop(&mut self) {
        PROGRESS.note(self.0);
    }
}

#[cfg(target_arch = "x86_64")]
mod failure {
    use super::super::progress::{FirstFailure, COUNT, NAMES, SITES};
    use super::*;
    use crate::task::thread::{Thread, ThreadState};
    use core::sync::atomic::Ordering;

    #[derive(Clone, Copy)]
    pub(crate) struct ThreadSample {
        pub tid: u64,
        pub pid: Option<u64>,
        pub owner: Option<usize>,
        pub state: Option<ThreadState>,
        pub current: bool,
        pub queued: bool,
        pub held: u64,
        pub name: [u8; 32],
        pub name_len: usize,
        pub name_cut: bool,
    }
    impl ThreadSample {
        pub const fn empty(tid: u64) -> Self {
            Self {
                tid,
                pid: None,
                owner: None,
                state: None,
                current: false,
                queued: false,
                held: 0,
                name: [0; 32],
                name_len: 0,
                name_cut: false,
            }
        }
        pub fn identify(&mut self, thread: &Thread) {
            self.pid = thread.owner_pid;
            self.state = Some(thread.state);
            self.name_len = thread.name.len().min(self.name.len());
            self.name[..self.name_len].copy_from_slice(&thread.name.as_bytes()[..self.name_len]);
            self.name_cut = thread.name.len() > self.name.len();
        }
    }

    #[derive(Clone, Copy)]
    struct Snapshot {
        counts: [u64; COUNT],
        sites: [u64; 3],
        rx_len: usize,
        tasks: usize,
        parked: Option<bool>,
        ns: u64,
        rows: [ThreadSample; 19],
        census_total: usize,
        last_accept: u64,
        last_ack: u64,
    }
    static FIRST: FirstFailure<Snapshot> = FirstFailure::new(Snapshot {
        counts: [0; COUNT],
        sites: [0; 3],
        rx_len: 0,
        tasks: 0,
        parked: None,
        ns: 0,
        rows: [ThreadSample::empty(0); 19],
        census_total: 0,
        last_accept: 0,
        last_ack: 0,
    });

    pub fn capture_overflow(rx_len: usize) {
        FIRST.capture(|snapshot| {
            // Freeze progress before taking bounded scheduler observations. These
            // are concurrent witnesses, not a globally atomic scheduler image.
            snapshot.counts = PROGRESS.sample();
            snapshot.sites = [COMMAND.site(), INPUT.site(), RENDER.site()];
            snapshot.last_accept = LAST_ACCEPT.load(Ordering::Relaxed);
            snapshot.last_ack = LAST_ACK.load(Ordering::Relaxed);
            snapshot.rx_len = rx_len;
            let (tid, tasks, parked) = crate::task::executor::input_snapshot();
            snapshot.tasks = tasks;
            snapshot.parked = parked;
            let (s, n) = crate::time::get_monotonic_time_ns();
            snapshot.ns = s.saturating_mul(1_000_000_000).saturating_add(n);
            let mut tids = [0; 16];
            snapshot.census_total = crate::task::dispatch_strand_census::outstanding_tids(&mut tids);
            for (row, tid) in snapshot.rows.iter_mut().zip(
                [
                    tid,
                    INPUT_TID.load(Ordering::Relaxed),
                    RENDER_TID.load(Ordering::Relaxed),
                ]
                .into_iter()
                .chain(tids),
            ) {
                if tid != 0 {
                    *row = crate::task::scheduler::input_thread_snapshot(tid);
                }
            }
        });
    }

    pub fn report_failure() {
        let Some(s) = FIRST.take() else {
            return;
        };
        // Called only by kstrandd with interrupts enabled, after all snapshot
        // guards were released. No allocation, no new logger or queue.
        crate::serial::enqueue_log(format_args!(
            "SERIAL_INPUT_STALL ns={} rx={} tasks={} parked={:?} command={} input={} render={} census_total={} accept={:016x} ack={:016x}\n",
            s.ns, s.rx_len, s.tasks, s.parked, SITES[s.sites[0] as usize],
            SITES[s.sites[1] as usize], SITES[s.sites[2] as usize], s.census_total,
            s.last_accept, s.last_ack));
        for (name, count) in NAMES.iter().zip(s.counts) {
            crate::serial::enqueue_log(format_args!("SERIAL_INPUT_PROGRESS {}={}\n", name, count));
        }
        for row in s.rows.iter().filter(|r| r.tid != 0) {
            crate::serial::enqueue_log(format_args!(
                "SERIAL_INPUT_THREAD tid={} pid={:?} owner={:?} state={:?} current={} queued={} held={:#x} name={} cut={}\n",
                row.tid, row.pid, row.owner, row.state, row.current, row.queued, row.held,
                core::str::from_utf8(&row.name[..row.name_len]).unwrap_or("<non-utf8>"), row.name_cut));
        }
    }
}
#[cfg(target_arch = "x86_64")]
pub(crate) use failure::{capture_overflow, report_failure, ThreadSample};
