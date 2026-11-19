//! Fixed atomic scheduler evidence for ARM64 BXCAP LOCKUP.

use core::sync::atomic::{fence, AtomicU64, Ordering};

pub(crate) const CPU_ROWS: usize = 8;
const WAIT_SLOTS: usize = 128;
pub(crate) const WAIT_ROWS: usize = 32;
const WORD_BITS: usize = u64::BITS as usize;
const NAME_WORDS: usize = 3;

/// Writers are serialized by the record's owner. A capture makes one read
/// attempt and omits an in-flight row, including if it interrupted the writer.
struct Sample<const N: usize> {
    version: AtomicU64,
    words: [AtomicU64; N],
}

impl<const N: usize> Sample<N> {
    const fn new() -> Self {
        Self {
            version: AtomicU64::new(0),
            words: [const { AtomicU64::new(0) }; N],
        }
    }

    fn store(&self, words: [u64; N]) {
        let version = self.version.load(Ordering::Relaxed);
        self.version
            .store(version.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        for (word, value) in self.words.iter().zip(words) {
            word.store(value, Ordering::Relaxed);
        }
        self.version
            .store(version.wrapping_add(2), Ordering::Release);
    }

    fn load(&self) -> Option<[u64; N]> {
        let before = self.version.load(Ordering::Acquire);
        if before & 1 != 0 {
            return None;
        }
        let words = self
            .words
            .each_ref()
            .map(|word| word.load(Ordering::Relaxed));
        // Seeing a word written after the writer's release fence also orders
        // the odd version before this final read, so a mixed row is rejected.
        fence(Ordering::Acquire);
        (before == self.version.load(Ordering::Relaxed)).then_some(words)
    }
}

static OWNER: Sample<3> = Sample::new();
static CURRENT: [AtomicU64; CPU_ROWS] = [const { AtomicU64::new(0) }; CPU_ROWS];
static WAITS: [Sample<7>; WAIT_SLOTS] = [const { Sample::new() }; WAIT_SLOTS];
static CLAIMED: [AtomicU64; WAIT_SLOTS / WORD_BITS] =
    [const { AtomicU64::new(0) }; WAIT_SLOTS / WORD_BITS];
static OMITTED: AtomicU64 = AtomicU64::new(0);

pub(crate) enum Owner {
    Free,
    Updating,
    Held { cpu: u64, pc: u64, tick: u64 },
}

pub(crate) fn acquired(cpu: usize, pc: u64, tick: u64) {
    OWNER.store([cpu as u64 + 1, pc, tick]);
}

pub(crate) fn releasing() {
    OWNER.store([0; 3]);
}

pub(crate) fn current(cpu: usize, tid: u64) {
    CURRENT[cpu].store(tid, Ordering::Relaxed);
}

/// Use a local instruction address so inlining places the PC at the caller's
/// acquisition or wait publication. Reading LR in Rust would report a
/// compiler-dependent return address after the last function call.
#[inline(always)]
pub(crate) fn site_pc() -> u64 {
    let pc: u64;
    unsafe {
        core::arch::asm!("adr {pc}, 2f", "2:", pc = out(reg) pc, options(nomem, nostack, preserves_flags));
    }
    pc
}

pub(crate) fn name_words(name: &str) -> [u64; NAME_WORDS] {
    let mut bytes = [0; NAME_WORDS * 8];
    let mut len = name.len().min(bytes.len());
    while !name.is_char_boundary(len) {
        len -= 1;
    }
    bytes[..len].copy_from_slice(&name.as_bytes()[..len]);
    core::array::from_fn(|i| u64::from_ne_bytes(bytes[i * 8..][..8].try_into().unwrap()))
}

/// The process table publishes the command name into its thread's signal
/// mirror. The wait producer copies it while it owns the thread.
pub(crate) struct CommandName(Sample<NAME_WORDS>);

impl Default for CommandName {
    fn default() -> Self {
        Self(Sample::new())
    }
}

impl CommandName {
    pub(crate) fn publish(&self, name: &str) {
        self.0.store(name_words(name));
    }

    pub(crate) fn words(&self) -> [u64; NAME_WORDS] {
        self.0.load().unwrap_or_default()
    }
}

/// Each blocked thread owns one atomic slot until it wakes or is destroyed.
/// The slot lives in static storage so a capture cannot race a thread free.
#[derive(Default)]
pub(crate) struct WaitSite {
    slot: Option<usize>,
    /// Queue identity handed to the I/O publisher within one scheduler hold.
    pub(super) queue_hint: u64,
}

impl WaitSite {
    pub(crate) fn publish(
        &mut self,
        tid: u64,
        pid: u64,
        pc: u64,
        queue: u64,
        name: [u64; NAME_WORDS],
    ) {
        let index = *self.slot.get_or_insert_with(|| {
            // The scheduler mutex serializes claims. A concurrent drop only
            // releases bits, and the release of a bit follows clearing its row.
            for (word_index, claimed) in CLAIMED.iter().enumerate() {
                let bit = (!claimed.load(Ordering::Acquire)).trailing_zeros() as usize;
                if bit < WORD_BITS {
                    claimed.fetch_or(1 << bit, Ordering::Relaxed);
                    return word_index * WORD_BITS + bit;
                }
            }
            OMITTED.fetch_add(1, Ordering::Relaxed);
            WAIT_SLOTS
        });
        if let Some(slot) = WAITS.get(index) {
            slot.store([tid, pid, pc, queue, name[0], name[1], name[2]]);
        }
    }

    pub(crate) fn set_pc(&self, pc: u64) {
        if let Some(slot) = self.slot.and_then(|index| WAITS.get(index)) {
            // This thread owns the slot and the scheduler serializes its writes.
            let mut words = slot
                .words
                .each_ref()
                .map(|word| word.load(Ordering::Relaxed));
            words[2] = pc;
            slot.store(words);
        }
    }

    pub(crate) fn clear(&mut self) {
        if let Some(index) = self.slot.take() {
            if let Some(slot) = WAITS.get(index) {
                slot.store([0; 7]);
                CLAIMED[index / WORD_BITS]
                    .fetch_and(!(1 << (index % WORD_BITS)), Ordering::Release);
            } else {
                OMITTED.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
}

impl Drop for WaitSite {
    fn drop(&mut self) {
        self.clear();
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct BlockedWait {
    pub tid: u64,
    pub pid: u64,
    pub site: u64,
    pub queue: u64,
    name: [u8; NAME_WORDS * 8],
}

impl BlockedWait {
    pub(crate) fn name(&self) -> &[u8] {
        let len = self
            .name
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(self.name.len());
        self.name.get(..len).unwrap_or_default()
    }
}

pub(crate) struct Snapshot {
    pub owner: Owner,
    pub current: [u64; CPU_ROWS],
    pub count: usize,
    pub omitted: u64,
    pub updating: u64,
}

pub(crate) fn snapshot(waits: &mut [BlockedWait; WAIT_ROWS]) -> Snapshot {
    let owner = match OWNER.load() {
        Some([0, _, _]) => Owner::Free,
        Some([cpu, pc, tick]) => Owner::Held {
            cpu: cpu - 1,
            pc,
            tick,
        },
        None => Owner::Updating,
    };
    let current = CURRENT.each_ref().map(|tid| tid.load(Ordering::Relaxed));
    let mut count = 0;
    let mut omitted = OMITTED.load(Ordering::Relaxed);
    let mut updating = 0;
    for slot in &WAITS {
        match slot.load() {
            Some([0, ..]) => {}
            Some([tid, pid, site, queue, a, b, c]) => {
                if let Some(wait) = waits.get_mut(count) {
                    *wait = BlockedWait {
                        tid,
                        pid,
                        site,
                        queue,
                        name: [0; NAME_WORDS * 8],
                    };
                    for (bytes, word) in wait.name.chunks_exact_mut(8).zip([a, b, c]) {
                        bytes.copy_from_slice(&word.to_ne_bytes());
                    }
                    count += 1;
                } else {
                    omitted += 1;
                }
            }
            None => updating += 1,
        }
    }
    Snapshot {
        owner,
        current,
        count,
        omitted,
        updating,
    }
}
