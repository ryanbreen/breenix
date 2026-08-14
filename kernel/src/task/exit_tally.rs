//! Userspace process exit accounting for the end-of-suite verdict.
//!
//! Failure details use a fixed-capacity table so recording an exit never
//! allocates. The failure table is taken under `PROCESS_MANAGER` while exits
//! are recorded, so it is a leaf lock: it must never be held with interrupts
//! enabled, and nothing may take `PROCESS_MANAGER` while holding it.

use core::fmt;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

const MAX_FAILURES: usize = 8;
const MAX_NAME_BYTES: usize = 24;

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

    fn new(name: &str, exit_code: i32) -> Self {
        let mut name_len = name.len().min(MAX_NAME_BYTES);
        while !name.is_char_boundary(name_len) {
            name_len -= 1;
        }

        let mut stored_name = [0; MAX_NAME_BYTES];
        stored_name[..name_len].copy_from_slice(&name.as_bytes()[..name_len]);

        Self {
            name: stored_name,
            name_len: name_len as u8,
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

/// Record one real userspace process exit.
pub fn record_exit(name: &str, exit_code: i32) {
    USERSPACE_EXITS.fetch_add(1, Ordering::SeqCst);

    if exit_code == 0 {
        return;
    }

    USERSPACE_NONZERO_EXITS.fetch_add(1, Ordering::SeqCst);
    with_failure_table_interrupts_disabled(|| {
        let mut failures = USERSPACE_FAILURES.lock();
        if failures.len < MAX_FAILURES {
            let index = failures.len;
            failures.failures[index] = ExitFailure::new(name, exit_code);
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

/// Copy the recorded failure details while holding their small static mutex.
pub fn snapshot_failures() -> ([ExitFailure; MAX_FAILURES], usize) {
    with_failure_table_interrupts_disabled(|| {
        let failures = USERSPACE_FAILURES.lock();
        (failures.failures, failures.len)
    })
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
