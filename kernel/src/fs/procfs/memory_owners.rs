//! Open-time ownership evidence, captured before text construction.
use alloc::string::String;
use core::fmt::Write;
use crate::memory::ownership_snapshot::{Row, Snapshot};

const CAPACITY: usize = 2048;
static SCRATCH: spin::Mutex<Snapshot<CAPACITY>> = spin::Mutex::new(Snapshot::new());

pub(super) fn generate() -> Result<String, i32> {
    // A concurrent reader receives EAGAIN rather than spinning on a preempted
    // formatter. Capture itself takes no heap or user-page allocations.
    let mut snapshot = SCRATCH.try_lock().ok_or(-11)?;
    snapshot.clear();
    crate::process::with_process_manager(|manager| {
        for (pid, process) in manager.iter_processes() {
            if snapshot.overflow { break; }
            let pid = pid.as_u64();
            let root = process.page_table.as_ref().map_or_else(
                || process.inherited_cr3.unwrap_or(0),
                |table| table.level_4_frame().start_address().as_u64());
            let mut row = Row { kind: "process", pid, root, ..Row::EMPTY };
            for (dst, src) in row.name.iter_mut().zip(process.name.bytes()) {
                *dst = if src.is_ascii_graphic() { src } else { b'_' };
            }
            snapshot.push(row);
            for vma in &process.vmas {
                if !snapshot.push(Row { kind: "vma", pid, root,
                    va: vma.start.as_u64(), end: vma.end.as_u64(),
                    flags: u64::from(vma.prot.bits()) | (u64::from(vma.flags.bits()) << 32),
                    ..Row::EMPTY }) { break; }
            }
            if let Some(table) = process.page_table.as_ref() {
                table.snapshot_ownership(pid, &mut snapshot);
            }
            for table in &process.pending_old_page_tables {
                if snapshot.overflow { break; }
                snapshot.push(Row { kind: "old-root", pid,
                    root: table.level_4_frame().start_address().as_u64(), ..Row::EMPTY });
                table.snapshot_ownership(pid, &mut snapshot);
            }
        }
    }).ok_or(-3)?;

    // Release PROCESS_MANAGER before constructing the text from stable rows.
    let mut text = String::new();
    text.try_reserve_exact(CAPACITY * 300).map_err(|_| -12)?;
    let _ = writeln!(text, "snapshot rows={} capacity={} overflow={}",
        snapshot.rows().len(), CAPACITY, u8::from(snapshot.overflow));
    for row in snapshot.rows() {
        let vmas = || snapshot.rows().iter().filter(|v| v.kind == "vma"
            && v.pid == row.pid && v.root == row.root && v.va < row.end && row.va < v.end);
        let first = vmas().next();
        let name_len = row.name.iter().position(|&b| b == 0).unwrap_or(row.name.len());
        let name = core::str::from_utf8(&row.name[..name_len]).unwrap_or("?");
        let _ = writeln!(text,
            "{} pid={} root={:#x} va={:#x} end={:#x} pa={:#x} level={} present={} nonzero={} flags={:#x} vmas={} first_vma={:#x}-{:#x} name={}",
            row.kind, row.pid, row.root, row.va, row.end, row.pa, row.level,
            row.present, row.nonzero, row.flags, vmas().count(),
            first.map_or(0, |v| v.va), first.map_or(0, |v| v.end), name);
    }
    Ok(text)
}
