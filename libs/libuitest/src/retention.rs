//! Bounded retention outside the supervisor's local collection reserve.
use crate::artifacts;
use breenix_suite::suite::{self, Run};
use libbreenix::{errno::Errno, error::Error, process, signal, time};
use std::{fs, path::Path};

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
    let end = now_ms()?.saturating_add(budget_ms);
    let mut send = || {
        if suite::upload_diagnostic(suite, label, bytes).is_ok() {
            0
        } else {
            1
        }
    };
    let pid = suite::spawn(&[], Run::Call(&mut send)).map_err(|e| e.to_string())?;
    let result = loop {
        let mut status = 0;
        match process::waitpid(pid.raw() as i32, &mut status, process::WNOHANG) {
            Ok(reaped) if reaped.raw() != 0 => {
                return if now_ms()? <= end
                    && process::wifexited(status)
                    && process::wexitstatus(status) == 0
                {
                    Ok(())
                } else {
                    Err("transfer".into())
                };
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
    println!("SUITE {suite} EVIDENCE bundle {case} label={label} bytes={size} sha256={hash} result={status} path={}", path.display());
    match result {
        Err(reason) if reason == "no-route" => Ok(()),
        other => other,
    }
}
