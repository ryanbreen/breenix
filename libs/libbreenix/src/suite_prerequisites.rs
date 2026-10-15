//! Measurement prerequisites shared by the effort suites.
//!
//! These return strings so the suite runner's `From<String> for CaseError` makes
//! missing coverage a failure. Passing a prerequisite never passes the case: the
//! caller still has to execute and check the operation being measured.

use std::{format, string::String};

/// CPU cases require simultaneous execution, not time slicing on fewer CPUs.
pub fn cpus(online: usize, required: usize) -> Result<(), String> {
    if online < required {
        Err(format!("missing CPU prerequisite: {online} processors online, requires at least {required} for simultaneous execution and the case's idle/controller processors"))
    } else {
        Ok(())
    }
}

pub fn priority_inheritance(offered: i64) -> Result<(), String> {
    if offered <= 0 {
        Err(format!("missing priority-inheritance prerequisite: sysconf(_SC_THREAD_PRIO_INHERIT) returned {offered}, requires a positive option value to measure PTHREAD_PRIO_INHERIT"))
    } else {
        Ok(())
    }
}

pub fn long_argument(max: u64) -> Result<(), String> {
    if max < 131072 {
        Err(format!("missing argument prerequisite: ARG_MAX={max}, requires at least 131072 bytes for the 65536-byte argument and near-limit exec measurement"))
    } else {
        Ok(())
    }
}

pub fn argument_allocation(max: u64, count: u64) -> Result<(), String> {
    if count > 2048 {
        Err(format!("unsupported E2BIG measurement: ARG_MAX={max} requires {count} arguments of 4096 bytes including NUL, exceeding the case's 8388608-byte allocation bound"))
    } else {
        Ok(())
    }
}

pub fn stack(soft: u64, depth_kib: usize) -> Result<(), String> {
    let required = 2 * 1024 * depth_kib as u64;
    if soft < required {
        Err(format!("missing stack prerequisite: RLIMIT_STACK={soft} bytes, requires at least {required} for the {depth_kib} KiB control before testing the 256 KiB limit"))
    } else {
        Ok(())
    }
}

pub fn invalid_semaphore_value(max: i64) -> Result<u32, String> {
    if !(0..i64::from(u32::MAX)).contains(&max) {
        Err(format!("missing invalid semaphore value: SEM_VALUE_MAX={max}, requires 0 <= SEM_VALUE_MAX < {} so SEM_VALUE_MAX+1 is representable by unsigned int", u32::MAX))
    } else {
        Ok(max as u32 + 1)
    }
}

/// Block allocation can witness writeback only if the stores did not allocate
/// all the required blocks already. Cached readback is not substitute evidence.
pub fn delayed_allocation(stored: i64, required: i64) -> Result<(), String> {
    if stored >= required {
        Err(format!("unsupported disk-writeback measurement: mapping stores already allocated {stored} blocks of {required} required before MS_SYNC; requires delayed allocation or an independent on-disk writeback observation, cached readback cannot prove writeback"))
    } else {
        Ok(())
    }
}

pub fn idle_measurement(idle_ms: i64, busy_ms: i64, wait_ms: i64, wait_idle_ms: i64, wait_measured: bool) -> Result<(), String> {
    if idle_ms < busy_ms || !wait_measured {
        Err(format!("missing idle-CPU measurement: computation had a spare processor for {idle_ms} ms, contended for {busy_ms} ms (requires idle >= contended); woken process wait={wait_ms} ms, idle processor time={wait_idle_ms} ms, spare-processor requirement met={wait_measured}"))
    } else {
        Ok(())
    }
}
