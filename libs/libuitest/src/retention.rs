//! Bounded retention outside the supervisor's local collection reserve.
use crate::artifacts;
use breenix_suite::suite::control::{Watchdog, Wait};
use breenix_suite::suite::{self, Run};
use libbreenix::{errno::Errno, error::Error, memory, process, signal, time};
use std::{fs, path::Path};
use std::sync::atomic::{AtomicU64, Ordering};

// Shared across fork: 0=not entered, 1=inside sender, 2=sender returned.
// The stamps remain enabled, and only a failed transfer publishes them.
struct TransferTrace(*mut AtomicU64);
impl TransferTrace {
    fn new() -> Result<Self, String> {
        let ptr = memory::mmap(core::ptr::null_mut(), 4096,
            memory::PROT_READ | memory::PROT_WRITE,
            memory::MAP_SHARED | memory::MAP_ANONYMOUS, -1, 0).map_err(|e| e.to_string())?
            .cast::<AtomicU64>();
        for index in 0..3 {
            // SAFETY: the new shared mapping holds at least three aligned cells.
            unsafe { ptr.add(index).write(AtomicU64::new(0)); }
        }
        Ok(Self(ptr))
    }
    fn at(&self, index: usize) -> &AtomicU64 {
        // SAFETY: callers name one of the three initialized cells; the mapping
        // outlives the child because transfer reaps it before dropping this.
        unsafe { &*self.0.add(index) }
    }
    fn mark(&self, phase: u64) {
        self.at(phase as usize).store(now_ms().unwrap_or(0), Ordering::Relaxed);
        self.at(0).store(phase, Ordering::Release);
    }
    fn report(&self, deadline: u64) {
        suite::value("retention-phase", self.at(0).load(Ordering::Acquire) as i64, "state", None);
        suite::value("retention-entered", self.at(1).load(Ordering::Relaxed) as i64, "ms", None);
        suite::value("retention-returned", self.at(2).load(Ordering::Relaxed) as i64, "ms", None);
        suite::value("retention-deadline", deadline as i64, "ms", None);
    }
}
impl Drop for TransferTrace {
    fn drop(&mut self) { let _ = memory::munmap(self.0.cast(), 4096); }
}

fn now_ms() -> Result<u64, String> {
    time::now_monotonic()
        .map(|t| (t.as_nanos() / 1_000_000) as u64)
        .map_err(|e| e.to_string())
}

// The existing diagnostic sender can block in lease, DNS, connect, send or read.
// Confine it to one owned child; the case process enforces one absolute deadline.
pub(crate) fn transfer(
    suite: &str,
    label: &str,
    bytes: &[u8],
    budget_ms: u64,
) -> Result<(), String> {
    let watchdog = Watchdog::new(Wait::DiagnosticUpload)?;
    let end = now_ms()?.saturating_add(budget_ms).min(watchdog.deadline_ms() as u64);
    let trace = TransferTrace::new()?;
    let mut send = || {
        trace.mark(1);
        let result = if suite::upload_diagnostic(suite, label, bytes).is_ok() {
            0
        } else {
            1
        };
        trace.mark(2);
        result
    };
    let pid = suite::spawn(&[], Run::Call(&mut send)).map_err(|e| e.to_string())?;
    let result = loop {
        let mut status = 0;
        match process::waitpid(pid.raw() as i32, &mut status, process::WNOHANG) {
            Ok(reaped) if reaped.raw() != 0 => {
                let result = if now_ms()? <= end
                    && process::wifexited(status)
                    && process::wexitstatus(status) == 0
                {
                    Ok(())
                } else {
                    Err("transfer".into())
                };
                // This branch consumed the exit; cleanup must not reap it twice.
                if result.is_err() { trace.report(end); }
                return result;
            }
            Ok(_) | Err(Error::Os(Errno::EINTR)) => {}
            Err(_) => break Err("wait".into()),
        }
        match now_ms() {
            Ok(now) if now < end => {}
            Ok(_) => break Err("deadline".into()),
            Err(_) => break Err("clock".into()),
        }
        if time::sleep_ms(1).is_err() {
            break Err("poll".into());
        }
    };
    // Only this function's recorded child is terminated and reaped.
    let _ = signal::kill(pid.raw() as i32, signal::SIGKILL);
    let mut status = 0;
    loop {
        match process::waitpid(pid.raw() as i32, &mut status, 0) {
            Err(Error::Os(Errno::EINTR)) => continue,
            _ => break,
        }
    }
    trace.report(end);
    result
}

/// Send the synced first-failure snapshot; leave local files in place on refusal.
/// Missing route wiring leaves disk evidence available to ARM launchers.
pub fn retain(suite: &str, case: &str) -> Result<(), String> {
    let label = artifacts::label(case);
    let path = Path::new("/var/uitest").join(suite).join(case);
    let bytes = fs::read(path.join("retention.bundle")).map_err(|_| "bundle-unavailable");
    let (size, hash) = bytes
        .as_ref()
        .map(|b| (b.len(), crate::frames::digest(b)))
        .unwrap_or((0, "none".into()));
    let result = (|| {
        if !Path::new("/etc/breenix/evidence-route").exists() {
            return Err("no-route".into());
        }
        let bytes = bytes?;
        if bytes.is_empty() || bytes.len() > artifacts::BUNDLE_LIMIT {
            return Err("bundle-bound".into());
        }
        let budget = artifacts::upload_budget_ms(suite::case_ms_left());
        if budget == 0 {
            return Err("case-deadline".into());
        }
        artifacts::reserve_upload(Path::new("/var/uitest"), &label)?;
        // Slot publication itself is local I/O; recalculate the remaining budget.
        let budget = budget.min(artifacts::upload_budget_ms(suite::case_ms_left()));
        if budget == 0 {
            return Err("case-deadline".into());
        }
        transfer(suite, &label, &bytes, budget)
    })();
    let status = match &result {
        Ok(()) => "ok".into(),
        Err(reason) => format!("disk-only:{reason}"),
    };
    suite::evidence_bundle(&label, size, &hash, &status, &path.to_string_lossy())?;
    match result {
        Err(reason) if reason == "no-route" => Ok(()),
        other => other,
    }
}
