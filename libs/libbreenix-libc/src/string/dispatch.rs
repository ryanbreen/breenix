//! Startup selects an immutable table. Calls before initialization are portable.

use super::{mem_asm, memchr_scalar, memcmp_scalar, strlen_scalar};
use core::sync::atomic::{AtomicPtr, Ordering};
use libbreenix::auxv::{AT_HWCAP, AT_HWCAP2};

type Copy = unsafe extern "C" fn(*mut u8, *const u8, usize) -> *mut u8;
type Compare = unsafe extern "C" fn(*const u8, *const u8, usize) -> i32;
type Find = unsafe extern "C" fn(*const u8, i32, usize) -> *mut u8;
type Len = unsafe extern "C" fn(*const u8) -> usize;

pub(super) struct Table {
    pub(super) copy: Copy,
    pub(super) move_bytes: Copy,
    pub(super) compare: Compare,
    pub(super) find: Find,
    pub(super) len: Len,
}

const PORTABLE: Table = Table {
    #[cfg(target_arch = "x86_64")]
    copy: mem_asm::copy_scalar,
    #[cfg(target_arch = "aarch64")]
    copy: mem_asm::memcpy,
    move_bytes: mem_asm::memmove,
    compare: memcmp_scalar,
    find: memchr_scalar,
    len: strlen_scalar,
};

static BASELINE: Table = PORTABLE;

#[cfg(target_arch = "x86_64")]
static FAST_REP: Table = Table {
    copy: mem_asm::copy_enhanced,
    ..PORTABLE
};

static SELECTED: AtomicPtr<Table> = AtomicPtr::new(core::ptr::null_mut());

fn select(_hwcap: u64, _hwcap2: u64) -> &'static Table {
    #[cfg(target_arch = "x86_64")]
    if _hwcap2 & libbreenix::auxv::x86::HWCAP2_ERMS != 0 {
        return &FAST_REP;
    }
    &BASELINE
}

pub(crate) fn initialize() {
    let hwcap = crate::startup_auxv::entry(AT_HWCAP).unwrap_or(0);
    let hwcap2 = crate::startup_auxv::entry(AT_HWCAP2).unwrap_or(0);
    let table = select(hwcap, hwcap2);
    let _ = SELECTED.compare_exchange(
        core::ptr::null_mut(),
        core::ptr::from_ref(table).cast_mut(),
        Ordering::Release,
        Ordering::Relaxed,
    );
}

#[inline]
pub(super) fn table() -> &'static Table {
    // SAFETY: only references to immutable static tables are published.
    unsafe { SELECTED.load(Ordering::Acquire).as_ref() }.unwrap_or(&BASELINE)
}
