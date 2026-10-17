//! Execute the production staging allocator with controlled frame and process services.
use std::{cell::{Cell, RefCell}, collections::BTreeMap, rc::Rc};

const PAGE: usize = 4096;
const MEMORY_QUOTA: u64 = 192 << 20;
const ENOMEM: i64 = 12;
const ENODEV: i64 = 19;
const EINVAL: i64 = 22;

#[derive(Default)]
struct Proc { bytes: u64 }
#[derive(Default)]
struct Core { procs: BTreeMap<u64, Proc> }
impl Core {
    fn proc_mut(&mut self, tgid: u64) -> &mut Proc { self.procs.entry(tgid).or_default() }
}
thread_local! {
    static CORE: RefCell<Core> = RefCell::new(Core::default());
    static LIMITS: Rc<Limits> = Rc::new(Limits::default());
    static FAIL_FRAMES: Cell<bool> = const { Cell::new(false) };
}
fn with_core<R>(f: impl FnOnce(&mut Core) -> R) -> Option<R> {
    Some(CORE.with(|c| f(&mut c.borrow_mut())))
}
#[derive(Default)]
struct Limits { held: Cell<u64>, ceiling: Cell<u64> }
impl Limits {
    fn release_gpu_bytes(&self, bytes: u64) { self.held.set(self.held.get().checked_sub(bytes).unwrap()); }
}
mod process {
    use super::*;
    pub struct ProcessId(u64);
    impl ProcessId { pub fn new(id: u64) -> Self { Self(id) } }
    pub struct Process { pub limits: Rc<Limits> }
    impl Process {
        pub fn reserve_gpu_bytes(&self, bytes: u64) -> bool {
            let next = self.limits.held.get() + bytes;
            if next > self.limits.ceiling.get() { return false; }
            self.limits.held.set(next);
            true
        }
    }
    pub struct Manager;
    impl Manager {
        pub fn get_process(&self, id: ProcessId) -> Option<Process> {
            (id.0 == 1).then(|| Process { limits: LIMITS.with(Rc::clone) })
        }
    }
    pub fn with_process_manager<R>(f: impl FnOnce(&Manager) -> R) -> Option<R> { Some(f(&Manager)) }
}
struct Pages { bytes: u64, limits: Option<Rc<Limits>> }
impl Pages {
    fn new(frames: usize) -> Option<Self> {
        if FAIL_FRAMES.with(Cell::get) { return None; }
        Some(Self { bytes: (frames * PAGE) as u64, limits: None })
    }
    fn charge_to(&mut self, limits: Rc<Limits>) { self.limits = Some(limits); }
}
impl Drop for Pages {
    fn drop(&mut self) {
        if let Some(limits) = &self.limits { limits.release_gpu_bytes(self.bytes); }
    }
}
#[path = "../kernel/src/gpu/staging.rs"]
mod staging;

fn setup(bytes: u64, limit: u64) {
    with_core(|c| { c.procs.clear(); c.proc_mut(1).bytes = bytes; });
    LIMITS.with(|l| { l.held.set(0); l.ceiling.set(limit); });
    FAIL_FRAMES.with(|f| f.set(false));
}
fn held() -> (u64, u64) {
    (with_core(|c| c.proc_mut(1).bytes).unwrap(), LIMITS.with(|l| l.held.get()))
}
#[test]
fn quota_refusal_preserves_resource_bytes_and_never_reserves_process_memory() {
    setup(MEMORY_QUOTA - PAGE as u64, u64::MAX);
    assert!(matches!(staging::Staging::new(1, PAGE + 1), Err(ENOMEM)));
    assert_eq!(held(), (MEMORY_QUOTA - PAGE as u64, 0));
}
#[test]
fn staging_counts_rounded_bytes_while_held_and_releases_both_charges() {
    let base = MEMORY_QUOTA - 3 * PAGE as u64;
    setup(base, u64::MAX);
    let first = staging::Staging::new(1, PAGE + 1).unwrap();
    assert_eq!(first.pages.bytes, 2 * PAGE as u64);
    assert_eq!(held(), (base + 2 * PAGE as u64, 2 * PAGE as u64));
    let second = staging::Staging::new(1, 1).unwrap();
    assert_eq!(held(), (MEMORY_QUOTA, 3 * PAGE as u64));
    assert!(matches!(staging::Staging::new(1, 1), Err(ENOMEM)));
    assert_eq!(held(), (MEMORY_QUOTA, 3 * PAGE as u64));
    drop(first);
    assert_eq!(held(), (base + PAGE as u64, PAGE as u64));
    drop(second);
    assert_eq!(held(), (base, 0));
}
#[test]
fn process_limit_and_frame_failure_roll_back_the_quota() {
    setup(8192, 0);
    assert!(matches!(staging::Staging::new(1, 1), Err(ENOMEM)));
    assert_eq!(held(), (8192, 0));
    LIMITS.with(|l| l.ceiling.set(u64::MAX));
    FAIL_FRAMES.with(|f| f.set(true));
    assert!(matches!(staging::Staging::new(1, 1), Err(ENOMEM)));
    assert_eq!(held(), (8192, 0));
    FAIL_FRAMES.with(|f| f.set(false));
    drop(staging::Staging::new(1, 1).unwrap());
    assert_eq!(held(), (8192, 0));
}
