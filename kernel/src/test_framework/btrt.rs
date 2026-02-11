//! Boot Test Result Table (BTRT) -- structured in-memory test results.
//!
//! The BTRT is a fixed-size table in BSS that the kernel populates during
//! boot. After all tests complete, the host extracts the table via QMP
//! `pmemsave`, producing reliable, timing-independent results.
//!
//! # Memory Layout
//!
//! ```text
//! BootTestResultTable (8240 bytes total):
//!   BtrtHeader   (48 bytes)   -- magic, counts, timestamps
//!   [BtrtEntry; 512] (8192 bytes) -- one per test slot
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use kernel::test_framework::btrt;
//!
//! btrt::init();              // Early in boot
//! btrt::pass(SERIAL_INIT);   // After serial works
//! btrt::fail(PCI_ENUM, BtrtErrorCode::NotFound, 0);
//! btrt::finalize();          // After all tests, emits BTRT_READY
//! ```

use super::catalog;
use super::ktap;
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};

/// Maximum number of test slots in the BTRT.
pub const MAX_TESTS: usize = 512;

/// BTRT magic value: "BTRT" + version 1.1
pub const BTRT_MAGIC: u64 = 0x4254_5254_0001_0001;

// =============================================================================
// Status and Error Enums
// =============================================================================

/// Test result status (wire protocol -- all variants required for host-side parser).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BtrtStatus {
    Pending = 0,
    Running = 1,
    Pass = 2,
    Fail = 3,
    Skip = 4,
    Timeout = 5,
}

/// Error code for failed tests (wire protocol -- all variants required for host-side parser).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BtrtErrorCode {
    Ok = 0,
    Panic = 1,
    Assert = 2,
    Timeout = 3,
    NotFound = 4,
    IoError = 5,
    Permission = 6,
    NoMemory = 7,
    NoExec = 8,
    Signal = 9,
    Deadlock = 10,
    Corrupt = 11,
    Unknown = 0xFF,
}

// =============================================================================
// Entry and Header Structs
// =============================================================================

/// A single test result entry (16 bytes, cache-line friendly).
#[derive(Clone, Copy)]
#[repr(C, align(16))]
pub struct BtrtEntry {
    /// Test ID from the catalog (0-511).
    pub test_id: u16,
    /// Result status.
    pub status: u8,
    /// Error code (only meaningful when status == Fail).
    pub error_code: u8,
    /// Test duration in microseconds.
    pub duration_us: u32,
    /// Error-specific detail value.
    pub error_detail: u32,
    /// Reserved for future use.
    pub reserved: u32,
}

impl BtrtEntry {
    const fn zeroed() -> Self {
        Self {
            test_id: 0,
            status: BtrtStatus::Pending as u8,
            error_code: BtrtErrorCode::Ok as u8,
            duration_us: 0,
            error_detail: 0,
            reserved: 0,
        }
    }
}

/// BTRT header (48 bytes).
#[repr(C)]
pub struct BtrtHeader {
    /// Magic value: `BTRT_MAGIC`.
    pub magic: u64,
    /// Total number of registered tests.
    pub total_tests: u32,
    /// Number of tests completed (atomic for concurrent updates).
    pub tests_completed: AtomicU32,
    /// Number of tests passed (atomic).
    pub tests_passed: AtomicU32,
    /// Number of tests failed (atomic).
    pub tests_failed: AtomicU32,
    /// Boot start timestamp (monotonic nanoseconds).
    pub boot_start_ns: u64,
    /// Boot end timestamp (monotonic nanoseconds, set by finalize).
    pub boot_end_ns: u64,
    /// Reserved.
    pub reserved: u64,
}

/// The complete Boot Test Result Table.
#[repr(C)]
pub struct BootTestResultTable {
    pub header: BtrtHeader,
    pub entries: [BtrtEntry; MAX_TESTS],
}

// =============================================================================
// Global Static
// =============================================================================

/// The global BTRT table, placed in BSS.
///
/// This is `#[no_mangle]` so GDB and QMP pmemsave can find it.
/// All access goes through raw pointers to avoid creating references to
/// `static mut` (which triggers UB warnings on recent nightly).
#[no_mangle]
static mut BTRT_TABLE: BootTestResultTable = BootTestResultTable {
    header: BtrtHeader {
        magic: 0, // Set to BTRT_MAGIC by init()
        total_tests: 0,
        tests_completed: AtomicU32::new(0),
        tests_passed: AtomicU32::new(0),
        tests_failed: AtomicU32::new(0),
        boot_start_ns: 0,
        boot_end_ns: 0,
        reserved: 0,
    },
    entries: [BtrtEntry::zeroed(); MAX_TESTS],
};

// =============================================================================
// Raw pointer helpers (avoid &BTRT_TABLE references)
// =============================================================================

/// Get a raw pointer to the BTRT table.
#[inline(always)]
fn table_ptr() -> *mut BootTestResultTable {
    // addr_of_mut! computes the address without creating a reference.
    addr_of_mut!(BTRT_TABLE)
}

/// Get a raw pointer to the header.
#[inline(always)]
fn header_ptr() -> *mut BtrtHeader {
    unsafe { addr_of_mut!((*table_ptr()).header) }
}

/// Get a raw pointer to an entry by index.
#[inline(always)]
fn entry_ptr(idx: usize) -> *mut BtrtEntry {
    unsafe { addr_of_mut!((*table_ptr()).entries[idx]) }
}

// =============================================================================
// Initialization
// =============================================================================

/// Initialize the BTRT table.
///
/// Must be called early in boot, before any `pass()`/`fail()` calls.
/// Prints the physical address to serial for QMP extraction.
pub fn init() {
    let hdr = header_ptr();
    // Safety: Called once during single-threaded early boot.
    unsafe {
        (*hdr).magic = BTRT_MAGIC;
        (*hdr).total_tests = catalog::CATALOG.len() as u32;
        (*hdr).boot_start_ns = boot_timestamp_ns();
    }

    let table_size = core::mem::size_of::<BootTestResultTable>();

    // Print physical address for QMP extraction.
    let virt_addr = table_ptr() as usize;
    let phys_addr = virt_to_phys(virt_addr);

    crate::serial_println!(
        "[btrt] Boot Test Result Table at phys {:#018x} ({} bytes)",
        phys_addr,
        table_size,
    );

    // Emit KTAP header
    ktap::emit_header(catalog::CATALOG.len() as u32);
}

// =============================================================================
// Recording API
// =============================================================================

/// Record a test result in the BTRT.
///
/// This is the core recording function. It:
/// 1. Writes the entry to the table
/// 2. Updates atomic counters in the header
/// 3. Emits a KTAP line to serial
/// 4. Emits a trace event (if tracing feature is available)
pub fn record(test_id: u16, status: BtrtStatus, error_code: BtrtErrorCode, error_detail: u32) {
    let idx = test_id as usize;
    if idx >= MAX_TESTS {
        return;
    }

    let name = catalog::test_name(test_id);

    // Write the entry via raw pointer. Each test_id is written exactly once.
    let entry = entry_ptr(idx);
    unsafe {
        (*entry).test_id = test_id;
        (*entry).status = status as u8;
        (*entry).error_code = error_code as u8;
        (*entry).error_detail = error_detail;
    }

    // Update header counters via raw pointer to AtomicU32 fields.
    let hdr = header_ptr();
    unsafe {
        (*hdr).tests_completed.fetch_add(1, Ordering::Release);
        match status {
            BtrtStatus::Pass => {
                (*hdr).tests_passed.fetch_add(1, Ordering::Release);
            }
            BtrtStatus::Fail | BtrtStatus::Timeout => {
                (*hdr).tests_failed.fetch_add(1, Ordering::Release);
            }
            _ => {}
        }
    }

    // Emit KTAP line (1-indexed sequence number for KTAP compatibility)
    let seq = test_id + 1;
    match status {
        BtrtStatus::Pass => ktap::emit_pass(seq, name),
        BtrtStatus::Fail => ktap::emit_fail(seq, name, error_code as u8, error_detail),
        BtrtStatus::Skip => ktap::emit_skip(seq, name),
        BtrtStatus::Timeout => ktap::emit_timeout(seq, name),
        _ => {}
    }

    // Emit trace event if tracing provider is available
    // TEMPORARILY DISABLED for debugging
    // #[cfg(feature = "btrt")]
    // emit_trace_event(test_id, status);
}

/// Record a passing test.
pub fn pass(test_id: u16) {
    record(test_id, BtrtStatus::Pass, BtrtErrorCode::Ok, 0);
}

/// Record a failing test.
pub fn fail(test_id: u16, error_code: BtrtErrorCode, error_detail: u32) {
    record(test_id, BtrtStatus::Fail, error_code, error_detail);
}

/// Record a skipped test.
pub fn skip(test_id: u16) {
    record(test_id, BtrtStatus::Skip, BtrtErrorCode::Ok, 0);
}

/// Record a timed-out test.
pub fn timeout(test_id: u16) {
    record(test_id, BtrtStatus::Timeout, BtrtErrorCode::Timeout, 0);
}

// =============================================================================
// Finalization
// =============================================================================

/// Guard to ensure finalize() is only executed once.
static BTRT_FINALIZED: AtomicBool = AtomicBool::new(false);

/// Finalize the BTRT after all tests complete.
///
/// Sets the boot_end_ns timestamp, emits the KTAP summary, and prints
/// the `===BTRT_READY===` sentinel that the host watches for.
///
/// Safe to call multiple times -- only the first call takes effect.
pub fn finalize() {
    if BTRT_FINALIZED.swap(true, Ordering::AcqRel) {
        return;
    }
    let hdr = header_ptr();

    unsafe {
        (*hdr).boot_end_ns = boot_timestamp_ns();
    }

    let (passed, failed, total, completed) = unsafe {
        (
            (*hdr).tests_passed.load(Ordering::Acquire),
            (*hdr).tests_failed.load(Ordering::Acquire),
            (*hdr).total_tests,
            (*hdr).tests_completed.load(Ordering::Acquire),
        )
    };

    let skipped = total.saturating_sub(completed);

    ktap::emit_summary(passed, failed, skipped);
    crate::serial_println!("===BTRT_READY===");
}

// =============================================================================
// PID Registry -- maps userspace PIDs to BTRT test IDs
// =============================================================================

/// Maximum number of tracked userspace test processes.
const MAX_PID_REGISTRY: usize = 128;

/// A single PID→test_id mapping slot. Lock-free via atomics.
struct PidRegistryEntry {
    /// Process ID (0 = empty slot).
    pid: AtomicU64,
    /// Corresponding BTRT test ID.
    test_id: AtomicU16,
}

impl PidRegistryEntry {
    const fn empty() -> Self {
        Self {
            pid: AtomicU64::new(0),
            test_id: AtomicU16::new(0),
        }
    }
}

/// Global PID registry (BSS-zeroed).
static BTRT_PID_REGISTRY: [PidRegistryEntry; MAX_PID_REGISTRY] = {
    // Work around const-init limitations: build array element by element.
    const EMPTY: PidRegistryEntry = PidRegistryEntry::empty();
    [EMPTY; MAX_PID_REGISTRY]
};

/// Number of PIDs registered so far.
static BTRT_REGISTERED_COUNT: AtomicU32 = AtomicU32::new(0);

/// Number of registered PIDs that have exited.
static BTRT_COMPLETED_COUNT: AtomicU32 = AtomicU32::new(0);

/// Register a userspace test process PID → test_id mapping.
///
/// Call this immediately after `create_user_process()` succeeds.
/// Also records the test as `Running` in the BTRT table.
pub fn register_pid(pid: u64, test_id: u16) {
    // Record the test as Running in BTRT
    record(test_id, BtrtStatus::Running, BtrtErrorCode::Ok, 0);

    // Claim the next empty slot
    let idx = BTRT_REGISTERED_COUNT.fetch_add(1, Ordering::AcqRel) as usize;
    if idx >= MAX_PID_REGISTRY {
        return; // Registry full -- test still tracked via BTRT entry
    }

    BTRT_PID_REGISTRY[idx].pid.store(pid, Ordering::Release);
    BTRT_PID_REGISTRY[idx].test_id.store(test_id, Ordering::Release);
}

/// Called when a userspace process exits. Looks up PID in the registry
/// and records pass (exit 0) or fail (exit non-zero) in the BTRT table.
///
/// If all registered test processes have completed, auto-finalizes.
pub fn on_process_exit(pid: u64, exit_code: i32) {
    let count = BTRT_REGISTERED_COUNT.load(Ordering::Acquire) as usize;
    let count = core::cmp::min(count, MAX_PID_REGISTRY);

    // Linear scan for matching PID
    for i in 0..count {
        let stored_pid = BTRT_PID_REGISTRY[i].pid.load(Ordering::Acquire);
        if stored_pid == pid {
            let test_id = BTRT_PID_REGISTRY[i].test_id.load(Ordering::Acquire);

            // Clear the slot so we don't match again (e.g. PID reuse)
            BTRT_PID_REGISTRY[i].pid.store(0, Ordering::Release);

            // Record pass or fail
            if exit_code == 0 {
                pass(test_id);
            } else {
                fail(test_id, BtrtErrorCode::Assert, exit_code as u32);
            }

            // Increment completed count and check for auto-finalize
            let completed = BTRT_COMPLETED_COUNT.fetch_add(1, Ordering::AcqRel) + 1;
            let registered = BTRT_REGISTERED_COUNT.load(Ordering::Acquire);
            if completed == registered && registered > 0 {
                finalize();
            }
            return;
        }
    }

    // PID not in registry -- forked child or non-test process, ignore.
}


// =============================================================================
// Helpers
// =============================================================================

/// Get a monotonic timestamp in raw ticks (TSC on x86_64, CNTVCT on ARM64).
fn boot_timestamp_ns() -> u64 {
    crate::arch_read_timestamp()
}

/// Convert a virtual address to a physical address.
fn virt_to_phys(virt: usize) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        // On x86_64, the kernel is loaded at a separate virtual offset from the HHDM,
        // so simple subtraction doesn't work for BSS addresses. Walk the page tables.
        crate::memory::PhysAddrWrapper::from_kernel_virt(virt) as usize
    }
    #[cfg(target_arch = "aarch64")]
    {
        // On ARM64, the kernel is mapped within the HHDM region, so simple
        // subtraction of the physical memory offset works.
        let offset = crate::memory::physical_memory_offset().as_u64() as usize;
        virt.wrapping_sub(offset)
    }
}

// Emit a trace event for a test result (only when tracing provider exists).
// TEMPORARILY DISABLED - boot_test provider removed for debugging
// #[cfg(feature = "btrt")]
// fn emit_trace_event(test_id: u16, status: BtrtStatus) {
//     use crate::tracing::providers::boot_test;
//     match status {
//         BtrtStatus::Pass => boot_test::trace_test_pass(test_id),
//         BtrtStatus::Fail => boot_test::trace_test_fail(test_id, 0),
//         BtrtStatus::Skip => boot_test::trace_test_skip(test_id),
//         BtrtStatus::Timeout => boot_test::trace_test_timeout(test_id),
//         BtrtStatus::Running => boot_test::trace_test_start(test_id),
//         BtrtStatus::Pending => {}
//     }
// }
