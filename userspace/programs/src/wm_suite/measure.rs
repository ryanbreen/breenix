//! Kernel completion receipts and the existing BGCLK001 UI-clock page.
use super::desktop::{now, until, Client, Desktop};
use libbreenix::{
    memory,
    syscall::{nr, raw},
};
use std::fs::OpenOptions;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub struct Clock {
    ptr: *mut u8,
    path: String,
}
impl Clock {
    pub fn new() -> Result<Self, String> {
        let path = format!("/tmp/wm-suite-clock-{}", std::process::id());
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let mut bytes = [0; 64];
        bytes[..8].copy_from_slice(b"BGCLK001");
        bytes[24..32].copy_from_slice(&1_767_225_600_000_000_000u64.to_le_bytes());
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.set_len(4096).map_err(|e| e.to_string())?;
        let ptr = memory::mmap(
            core::ptr::null_mut(),
            4096,
            memory::PROT_READ | memory::PROT_WRITE,
            memory::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
        .map_err(|e| e.to_string())?;
        std::env::set_var("BREENIX_UI_CLOCK", &path);
        Ok(Self { ptr, path })
    }
    pub fn step(&mut self, epoch: u64, ns: u64) -> Result<(), String> {
        // SAFETY: the shared file mapping is page-aligned, writable and lives through this call.
        let result = unsafe {
            (&*self.ptr.add(16).cast::<AtomicU64>()).store(ns, Ordering::Relaxed);
            (&*self.ptr.add(8).cast::<AtomicU64>()).store(epoch, Ordering::Release);
            (&*self.ptr.add(32).cast::<AtomicU32>()).fetch_add(1, Ordering::Release);
            raw::syscall6(
                nr::FUTEX,
                self.ptr.add(32) as u64,
                1,
                i32::MAX as u64,
                0,
                0,
                0,
            ) as i64
        };
        if result < 0 {
            return Err(format!(
                "shared UI-clock FUTEX_WAKE failed: errno {}",
                -result
            ));
        }
        Ok(())
    }
}
impl Drop for Clock {
    fn drop(&mut self) {
        std::env::remove_var("BREENIX_UI_CLOCK");
        let _ = memory::munmap(self.ptr, 4096);
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn acknowledged(epoch: u64) -> Result<(), String> {
    until(2000, "bwm acknowledged UI-clock epoch", || {
        let status = std::fs::read_to_string("/proc/breenix/compositor").map_err(|e| {
            format!("bwm status/epoch acknowledgment /proc/breenix/compositor is absent: {e}")
        })?;
        let got = status
            .split_whitespace()
            .find_map(|s| s.strip_prefix("epoch=").and_then(|s| s.parse::<u64>().ok()))
            .ok_or("compositor status omitted its acknowledged epoch")?;
        Ok(got == epoch)
    })
}

#[derive(Clone, Copy)]
pub struct Receipt {
    pub serial: u64,
    pub ns: u64,
    pub window: u64,
    pub commit: u64,
    pub generation: u64,
}
pub fn receipts() -> Result<Vec<Receipt>, String> {
    let text = std::fs::read_to_string("/proc/breenix/composites").map_err(|e| {
        format!("kernel completed-composite receipts /proc/breenix/composites are absent: {e}")
    })?;
    text.lines()
        .map(|s| {
            let fields = s
                .split_whitespace()
                .map(str::parse::<u64>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            if fields.len() != 5 || fields[0] == 0 || fields[1] == 0 {
                return Err("malformed completed-composite receipt".into());
            }
            Ok(Receipt {
                serial: fields[0],
                ns: fields[1],
                window: fields[2],
                commit: fields[3],
                generation: fields[4],
            })
        })
        .collect()
}
/// Kernel device-completion history supplies timing without observer readbacks per sample.
/// The caller still validates the final completed scanout after the measured workload.
pub fn completed_commit(c: &Client, commit: u64) -> Result<Receipt, String> {
    if commit == 0 {
        return Err("no input handling commit".into());
    }
    let mut found = None;
    until(2000, "device-completed target commit", || {
        found = receipts()?
            .into_iter()
            .filter(|r| r.window == c.id && r.commit >= commit)
            .min_by_key(|r| r.serial);
        Ok(found.is_some())
    })?;
    let r = found.unwrap();
    if r.ns > now() || r.generation == 0 {
        return Err("completion receipt has no completed pixel generation".into());
    }
    Ok(r)
}
/// A final native observation must cover the receipts and client commits judged above.
pub fn observed(d: &mut Desktop, clients: &[&Client], serial: u64) -> Result<(), String> {
    let last = clients.last().ok_or("no clients in final observation")?;
    let screen = d.shown(last)?;
    if screen.serial() < serial || !clients.iter().all(|c| screen.contains(c.id, c.commit)) {
        return Err("final device observation does not cover workload receipts and commits".into());
    }
    Ok(())
}

pub fn epoch(
    clock: &mut Clock,
    clients: &mut [Client],
    n: u32,
    animate: bool,
) -> Result<u64, String> {
    clock.step(n as u64, n as u64 * 16_000_000)?;
    for c in clients.iter_mut() {
        c.command(super::protocol::STEP, n, u32::from(animate))?;
    }
    acknowledged(n as u64)?;
    let mut serial = 0;
    for c in clients.iter() {
        serial = serial.max(completed_commit(c, c.commit)?.serial);
    }
    Ok(serial)
}
