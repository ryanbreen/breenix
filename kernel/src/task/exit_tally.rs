//! Userspace process exit accounting for the end-of-suite verdict.
//!
//! Failure details use a fixed-capacity table so recording an exit never
//! allocates. The failure table is taken under `PROCESS_MANAGER` while exits
//! are recorded, so it is a leaf lock: it must never be held with interrupts
//! enabled, and nothing may take `PROCESS_MANAGER` while holding it.

use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

// Test profiles retain up to 1024 nonzero exits. Production retains 8.
// FailureList prints an ellipsis when the retained table is incomplete.
#[cfg(any(feature = "testing", feature = "boot_tests"))]
const MAX_FAILURES: usize = 1024;
#[cfg(not(any(feature = "testing", feature = "boot_tests")))]
const MAX_FAILURES: usize = 8;
const MAX_NAME_BYTES: usize = 128;

static USERSPACE_STARTED: AtomicU32 = AtomicU32::new(0);
static USERSPACE_WITHDRAWN: AtomicU32 = AtomicU32::new(0);
static USERSPACE_EXITS: AtomicU32 = AtomicU32::new(0);
static USERSPACE_NONZERO_EXITS: AtomicU32 = AtomicU32::new(0);
static USERSPACE_FAILURES: Mutex<FailureTable> = Mutex::new(FailureTable::new());

#[cfg(target_arch = "x86_64")]
fn with_failure_table_interrupts_disabled<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    x86_64::instructions::interrupts::without_interrupts(f)
}

#[cfg(target_arch = "aarch64")]
fn with_failure_table_interrupts_disabled<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    crate::arch_impl::aarch64::cpu::without_interrupts(f)
}

#[derive(Clone, Copy)]
pub struct ExitFailure {
    name: [u8; MAX_NAME_BYTES],
    name_len: u8,
    exit_code: i32,
}

impl ExitFailure {
    const EMPTY: Self = Self {
        name: [0; MAX_NAME_BYTES],
        name_len: 0,
        exit_code: 0,
    };

    /// `name`, or `<name>_child_<pid>` for a forked child that has not
    /// exec'd: it shares its parent's name, but the verdict must not count
    /// its exit as the parent program's.
    fn new(name: &str, forked_child: Option<u64>, exit_code: i32) -> Self {
        let mut suffix = [0u8; 27];
        let mut suffix_len = 0;
        if let Some(pid) = forked_child {
            suffix[..7].copy_from_slice(b"_child_");
            suffix_len = 7;
            let mut digits = [0u8; 20];
            let mut count = 0;
            let mut value = pid;
            loop {
                digits[count] = b'0' + (value % 10) as u8;
                count += 1;
                value /= 10;
                if value == 0 {
                    break;
                }
            }
            for digit in digits[..count].iter().rev() {
                suffix[suffix_len] = *digit;
                suffix_len += 1;
            }
        }

        let mut name_len = name.len().min(MAX_NAME_BYTES - suffix_len);
        while !name.is_char_boundary(name_len) {
            name_len -= 1;
        }

        let mut stored_name = [0; MAX_NAME_BYTES];
        stored_name[..name_len].copy_from_slice(&name.as_bytes()[..name_len]);
        stored_name[name_len..name_len + suffix_len].copy_from_slice(&suffix[..suffix_len]);

        Self {
            name: stored_name,
            name_len: (name_len + suffix_len) as u8,
            exit_code,
        }
    }

    fn name(&self) -> &str {
        // Names are truncated on a UTF-8 character boundary, so this fallback is defensive only.
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("<unprintable>")
    }
}

struct FailureTable {
    failures: [ExitFailure; MAX_FAILURES],
    len: usize,
}

impl FailureTable {
    const fn new() -> Self {
        Self {
            failures: [ExitFailure::EMPTY; MAX_FAILURES],
            len: 0,
        }
    }
}

/// Count a process published into the manager, including fork/clone children.
pub fn record_start() {
    USERSPACE_STARTED.fetch_add(1, Ordering::SeqCst);
}

/// Take back the start of a published row removed from the process table
/// before it terminated: exits are recorded by `terminate` and
/// `terminate_minimal`, and the removed row reaches neither. The in-kernel
/// teardown oracles' rows and a failed init publication's row leave this way.
pub fn withdraw_start() {
    USERSPACE_STARTED.fetch_sub(1, Ordering::SeqCst);
    USERSPACE_WITHDRAWN.fetch_add(1, Ordering::SeqCst);
}

/// Published processes that have not been withdrawn: each records one exit.
pub fn started() -> u32 {
    USERSPACE_STARTED.load(Ordering::SeqCst)
}

/// Published rows withdrawn without an exit.
pub fn withdrawn() -> u32 {
    USERSPACE_WITHDRAWN.load(Ordering::SeqCst)
}

/// Record one real userspace process exit; `forked_child` is the pid of a
/// forked child that has not exec'd.
pub fn record_exit(name: &str, forked_child: Option<u64>, exit_code: i32) {
    USERSPACE_EXITS.fetch_add(1, Ordering::SeqCst);

    if exit_code == 0 {
        return;
    }

    USERSPACE_NONZERO_EXITS.fetch_add(1, Ordering::SeqCst);
    with_failure_table_interrupts_disabled(|| {
        let mut failures = USERSPACE_FAILURES.lock();
        if failures.len < MAX_FAILURES {
            let index = failures.len;
            failures.failures[index] = ExitFailure::new(name, forked_child, exit_code);
            failures.len += 1;
        }
    });
}

/// Return the total userspace exits and nonzero userspace exits.
pub fn totals() -> (u32, u32) {
    (
        USERSPACE_EXITS.load(Ordering::SeqCst),
        USERSPACE_NONZERO_EXITS.load(Ordering::SeqCst),
    )
}

/// Allocate the snapshot before acquiring the leaf lock.
/// Called only by the completion reporter, after dropping PROCESS_MANAGER.
pub fn snapshot_failures() -> Vec<ExitFailure> {
    let mut snapshot = Vec::with_capacity(MAX_FAILURES);
    with_failure_table_interrupts_disabled(|| {
        let failures = USERSPACE_FAILURES.lock();
        snapshot.extend_from_slice(&failures.failures[..failures.len]);
    });
    snapshot
}

/// Allocation-free formatting for the recorded `name:code` failure list.
pub struct FailureList<'a> {
    failures: &'a [ExitFailure],
    truncated: bool,
}

impl<'a> FailureList<'a> {
    pub fn new(failures: &'a [ExitFailure], total_nonzero: u32) -> Self {
        Self {
            failures,
            truncated: total_nonzero as usize > failures.len(),
        }
    }
}

impl fmt::Display for FailureList<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, failure) in self.failures.iter().enumerate() {
            if index != 0 {
                formatter.write_str(",")?;
            }
            write!(formatter, "{}:{}", failure.name(), failure.exit_code)?;
        }

        if self.truncated {
            if !self.failures.is_empty() {
                formatter.write_str(",")?;
            }
            formatter.write_str("...")?;
        }

        Ok(())
    }
}
