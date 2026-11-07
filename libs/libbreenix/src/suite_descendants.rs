//! Descendant scope for a non-init subreaper. PID 1 keeps the original runner sweep.
use crate::{error::Error, errno::Errno, process, signal, time};
use std::vec::Vec;

/// Select only rows whose parent chain reaches this runner. An adopted orphan
/// has the runner as its parent. Parent walks stop at missing rows or the table-length bound.
/// PID 1 is deliberately excluded: its existing sweep is a separate path.
pub fn descendant_victims(self_pid: u64, parents: &[(u64, u64)]) -> Vec<u64> {
    if self_pid <= 1 { return Vec::new(); }
    parents.iter().filter_map(|&(pid, parent)| {
        if pid <= 1 || pid == self_pid { return None; }
        let mut ancestor = parent;
        for _ in 0..parents.len() {
            if ancestor == self_pid { return Some(pid); }
            if ancestor <= 1 { break; }
            match parents.iter().find(|&&(id, _)| id == ancestor) {
                Some(&(_, next)) => ancestor = next,
                None => break,
            }
        }
        None
    }).collect()
}

fn parent_table() -> Result<Vec<(u64, u64)>, ()> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir("/proc").map_err(|_| ())? {
        let entry = entry.map_err(|_| ())?;
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u64>().ok()) else { continue };
        let stat = match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(()),
        };
        // comm may itself contain spaces or ')': the state follows the final ') '.
        let parent = stat.rsplit_once(") ").and_then(|(_, rest)| rest.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok()).ok_or(())?;
        rows.push((pid, parent));
    }
    Ok(rows)
}

/// Kill and reap this non-PID-1 subreaper's descendants within the runner's 2 s
/// sweep bound. Rescan on each kill pass so reparenting and late forks are covered.
/// The caller must have set PR_SET_CHILD_SUBREAPER before starting its children.
pub fn sweep_descendants() -> Option<&'static str> {
    let pid = match process::getpid() {
        Ok(pid) if pid.raw() > 1 => pid.raw(),
        _ => return Some("descendant sweep requires a non-PID-1 subreaper"),
    };
    let start = match time::now_monotonic() {
        Ok(now) => now.as_nanos(),
        Err(_) => return Some("descendant sweep could not read the monotonic clock"),
    };
    let mut killed = None;
    loop {
        let elapsed = match time::now_monotonic() {
            Ok(now) => now.as_nanos().saturating_sub(start) / 1_000_000,
            Err(_) => return Some("descendant sweep could not read the monotonic clock"),
        };
        if killed.is_none_or(|at| elapsed >= at + super::SWEEP_REKILL_MS as i128) {
            let rows = match parent_table() {
                Ok(rows) => rows,
                Err(()) => return Some("descendant sweep could not read the parent table"),
            };
            for victim in descendant_victims(pid, &rows) {
                let _ = signal::kill(victim as i32, signal::SIGKILL);
            }
            killed = Some(elapsed);
        }
        loop {
            let mut status = 0;
            match process::waitpid(-1, &mut status, super::WNOHANG) {
                Ok(child) if child.raw() != 0 => continue,
                Ok(_) => break,
                Err(Error::Os(Errno::EINTR)) => continue,
                Err(Error::Os(Errno::ECHILD)) => return None,
                Err(_) => return Some("descendant sweep could not reap children"),
            }
        }
        if elapsed >= super::SWEEP_MS as i128 {
            return Some("processes it started outlived SIGKILL");
        }
        let _ = process::yield_now();
    }
}
