//! Native-only live small-allocation census for retained failure-reason accounting.
//! Callbacks use fixed storage, made resident before the memory baseline.
use std::{cell::UnsafeCell, fmt::Write, sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering}};
const SLOTS: usize = 4096;
const PREFIX: usize = 160;
const MAX_BYTES: usize = 1 << 20;
#[derive(Clone, Copy)]
struct Slot { pointer: usize, size: usize, align: usize, birth: usize }
impl Slot { const EMPTY: Self = Self { pointer: 0, size: 0, align: 0, birth: 0 }; }
struct Table { slots: [Slot; SLOTS], phase: usize }
impl Table {
    const fn new() -> Self { Self { slots: [Slot::EMPTY; SLOTS], phase: 0 } }
    fn hash(pointer: usize) -> usize {
        let value = (pointer >> 4).wrapping_mul(0x9e3779b97f4a7c15);
        (value ^ (value >> 32)) & (SLOTS - 1)
    }
    fn insert(&mut self, slot: Slot) -> bool {
        let first = Self::hash(slot.pointer);
        for n in 0..SLOTS {
            let at = (first + n) & (SLOTS - 1);
            if self.slots[at].pointer == 0 || self.slots[at].pointer == slot.pointer {
                self.slots[at] = slot;return true;
            }
        }
        false
    }
    fn remove(&mut self, pointer: usize) -> bool {
        let first = Self::hash(pointer);
        for n in 0..SLOTS {
            let at = (first + n) & (SLOTS - 1);
            if self.slots[at].pointer == 0 { return false; }
            if self.slots[at].pointer != pointer { continue; }
            self.slots[at] = Slot::EMPTY;
            // Rehash the cluster, so a long boot does not accumulate tombstones.
            let mut next = (at + 1) & (SLOTS - 1);
            for _ in 0..SLOTS {
                let moved = self.slots[next];
                if moved.pointer == 0 { break; }
                self.slots[next] = Slot::EMPTY;
                assert!(self.insert(moved));
                next = (next + 1) & (SLOTS - 1);
            }
            return true;
        }
        false
    }
}
struct Storage(UnsafeCell<Table>);
// SAFETY: LOCK guards the table; prepare runs before arming and encode disarms it.
unsafe impl Sync for Storage {}
static DATA: Storage = Storage(UnsafeCell::new(Table::new()));
static LOCK: AtomicBool = AtomicBool::new(false);
static ARMED: AtomicBool = AtomicBool::new(false);
static OWNER: AtomicU32 = AtomicU32::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);
struct Held;
impl Drop for Held { fn drop(&mut self) { LOCK.store(false, Ordering::Release); } }
fn with_table<R>(f: impl FnOnce(&mut Table) -> R) -> Option<R> {
    if LOCK.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        DROPPED.fetch_add(1, Ordering::Relaxed);return None;
    }
    let _held = Held;
    // SAFETY: the guard provides exclusive access, with no allocator recursion.
    Some(f(unsafe { &mut *DATA.0.get() }))
}
fn tracking() -> bool {
    if !ARMED.load(Ordering::Acquire) { return false; }
    // fork copies the census. Only the runner parent owns retained reasons;
    // a case disarms its private copy on its first allocator callback.
    if std::process::id() != OWNER.load(Ordering::Relaxed) {
        ARMED.store(false, Ordering::Release);return false;
    }
    true
}
fn eligible(size: usize, align: usize) -> bool { size > 0 && size <= 2048 && align > 0 && align <= 16 && align <= size }
pub(super) fn prepare() -> Result<(), String> {
    if ARMED.load(Ordering::Acquire) { return Err("allocation census already armed".into()); }
    with_table(|table| {
        // Volatile initialization commits first-touch backing before the baseline.
        for slot in &mut table.slots { unsafe { std::ptr::write_volatile(slot, Slot::EMPTY); } }
        table.phase = 0;
    }).ok_or("allocation census preparation lock")?;
    DROPPED.store(0, Ordering::Relaxed);OWNER.store(std::process::id(), Ordering::Relaxed);
    ARMED.store(true, Ordering::Release);Ok(())
}
pub(super) fn allocated(pointer: *mut u8, size: usize, align: usize) {
    if pointer.is_null() || !eligible(size, align) || !tracking() { return; }
    let _ = with_table(|table| {
        if !table.insert(Slot { pointer: pointer as usize, size, align, birth: table.phase }) {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
    });
}
pub(super) fn deallocated(pointer: *mut u8, size: usize, align: usize) {
    if pointer.is_null() || !eligible(size, align) || !tracking() { return; }
    let _ = with_table(|table| { table.remove(pointer as usize); });
}
pub(super) fn reallocated(old: *mut u8, old_size: usize, align: usize, new: *mut u8, size: usize) {
    // A failed realloc retains the original allocation and its evidence.
    if new.is_null() || !tracking() { return; }
    let _ = with_table(|table| {
        if eligible(old_size, align) { table.remove(old as usize); }
        if eligible(size, align) && !table.insert(Slot { pointer: new as usize, size, align, birth: table.phase }) {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
    });
}
pub(super) fn boundary(phase: usize) {
    if ARMED.load(Ordering::Acquire) { let _ = with_table(|table| { table.phase = phase; }); }
}
pub(super) fn encode() -> Result<Vec<u8>, String> {
    #[cfg(target_os = "breenix")]
    {
        let path = format!("/proc/{}/task", std::process::id());
        let mut threads = 0;
        for entry in std::fs::read_dir(path).map_err(|_| "allocation census task directory")? {
            entry.map_err(|_| "allocation census task entry")?;threads += 1;
        }
        if threads != 1 { return Err("allocation census requires one runner thread".into()); }
    }
    if !ARMED.swap(false, Ordering::AcqRel) { return Err("allocation census is not armed".into()); }
    with_table(|table| {
        let count = table.slots.iter().filter(|s| s.pointer != 0).count();
        let mut text = format!("GT710_BREENGL_ALLOCATIONS {{\"schema\":1,\"phase\":{},\"dropped\":{},\"count\":{count},\"entries\":[",
            table.phase, DROPPED.load(Ordering::Relaxed));
        for (index, slot) in table.slots.iter().filter(|s| s.pointer != 0).enumerate() {
            write!(text, "{}[{},{},{},{},\"", if index == 0 { "" } else { "," },
                slot.pointer, slot.size, slot.align, slot.birth).map_err(|_| "allocation census formatting")?;
            // SAFETY: the table contains live allocations only, and the snapshot
            // is taken by the single runner thread after disarming hooks. Its
            // Breenix malloc mappings are zero-initialized, including padding;
            // reused mappings retain initialized bytes. No allocation is freed
            // while this synchronous snapshot reads its bounded byte prefix.
            let prefix = unsafe { std::slice::from_raw_parts(slot.pointer as *const u8, slot.size.min(PREFIX)) };
            for byte in prefix { write!(text, "{byte:02x}").map_err(|_| "allocation prefix formatting")?; }
            text.push_str("\"]");
            if text.len() > MAX_BYTES { return Err("allocation census exceeds bound".into()); }
        }
        text.push_str("]}\n");Ok(text.into_bytes())
    }).ok_or("allocation census snapshot lock")?
}
