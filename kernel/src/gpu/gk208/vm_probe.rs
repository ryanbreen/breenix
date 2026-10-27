/*
 * Copyright 2012 Red Hat Inc.
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sublicense,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
 * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 *
 * Authors: Ben Skeggs
 */
//! M5.2 CPU-only acceptance: private tables are not scheduled on any engine.
//! BAR1 physical viewing calls the attributed M4 implementation. Fault reads
//! and VM encodings call vm.rs; no inherited VRAM storage is written here.
use super::{
    inventory::{digest, pattern, stats, words},
    lifetime::{Consumer, Context},
    memory::{self, allocator::Kind, Range},
    modeset,
    vm::{self, Flags, Io, Space},
    vm_control, Registers,
};
use crate::{arch_impl::x86_64::timer, drivers::pci};
use alloc::{format, string::String, vec, vec::Vec};
use core::fmt::Write;
const VA: u64 = (128 << 20) - 4096;

struct DeviceIo<'a> {
    p: memory::Pramin<'a, Registers>,
    invalidations: Vec<[u64; 4]>,
}
impl Io for DeviceIo<'_> {
    fn read(&mut self, a: u64) -> Result<u64, &'static str> {
        Ok(u64::from(self.p.read(a)?) | (u64::from(self.p.read(a + 4)?) << 32))
    }
    fn write(&mut self, a: u64, v: u64) -> Result<(), &'static str> {
        self.p.write(a, v as u32)?;
        self.p.write(a + 4, (v >> 32) as u32)
    }
    fn flush(&mut self) -> Result<(), &'static str> {
        modeset::bar_flush(self.p.access())
    }
    fn invalidate(&mut self, pd: u64) -> Result<u32, &'static str> {
        let status = vm_control::invalidate(self.p.access(), pd)?;
        self.invalidations
            .push([pd, pd >> 8, 0x80000001, u64::from(status)]);
        Ok(status)
    }
    fn register(&mut self, r: usize) -> u32 {
        self.p.access().read(r)
    }
}
fn fault_json(records: &[vm::Fault]) -> String {
    let mut s = String::new();
    for r in records {
        if !s.is_empty() {
            s.push(',');
        }
        let _=write!(s,"{{\"unit\":{},\"raw\":{:?},\"instance\":{},\"address\":{},\"reason\":{},\"client\":{},\"gpc\":{},\"hub\":{},\"write\":{}}}",
            r.unit,r.raw,r.instance,r.address,r.reason,r.client,r.gpc,r.hub,r.write);
    }
    s
}
/// Reuse the sole existing WC CPU alias. The private viewing table is itself
/// charged VRAM, independent of either address space's tables.
fn view(regs: &mut Registers, total: u64, window: Range, r: Range) -> Result<(), &'static str> {
    let (pde, ptes) = modeset::bar1_small_pages(r.start, r.size, window.start + 8192)?;
    words(
        regs,
        total,
        window.start + 0x200,
        &[
            (window.start + 4096) as u32,
            0,
            (modeset::BAR1_WINDOW - 1) as u32,
            0,
        ],
    )?;
    words(regs, total, window.start + 4096, &pde)?;
    let mut p = memory::Pramin::new(regs, total);
    for (i, pte) in ptes.into_iter().enumerate() {
        p.write(window.start + 8192 + i as u64 * 8, pte as u32)?;
        p.write(window.start + 8196 + i as u64 * 8, (pte >> 32) as u32)?;
    }
    drop(p);
    modeset::activate_bar1(regs, window.start, window.start + 4096)?;
    Ok(())
}
fn checked_bytes(
    regs: &mut Registers,
    total: u64,
    window: Range,
    pointer: usize,
    r: Range,
) -> Result<Vec<u8>, &'static str> {
    view(regs, total, window, r)?;
    let actual = memory::Pramin::new(regs, total).bytes(r)?;
    for (i, bytes) in actual.chunks_exact(4).enumerate() {
        // SAFETY: view() installed this bounded physical allocation on BAR1.
        let bar = unsafe { ((pointer + i * 4) as *const u32).read_volatile() };
        if bar.to_le_bytes() != bytes {
            return Err("VM independent table aperture mismatch");
        }
    }
    Ok(actual)
}
fn snapshot(
    io: &mut DeviceIo<'_>,
    total: u64,
    window: Range,
    pointer: usize,
    space: &Space,
    index: usize,
    phase: &str,
    backing: Option<u64>,
    bits: u64,
) -> Result<String, &'static str> {
    let mut binding = vec![0u8; 4096];
    binding[0x200..0x208].copy_from_slice(&space.directory.range.start.to_le_bytes());
    binding[0x208..0x210].copy_from_slice(&(vm::LIMIT - 1).to_le_bytes());
    if checked_bytes(io.p.access(), total, window, pointer, space.instance.range)? != binding {
        return Err("VM instance binding snapshot mismatch");
    }
    let mut directory = vec![0u8; 65536];
    let mut tables = String::new();
    let mut hashes = String::new();
    for (pdi, r) in space.table_ranges() {
        let pde = ((r.start >> 8) | 1) << 32;
        directory[pdi as usize * 8..pdi as usize * 8 + 8].copy_from_slice(&pde.to_le_bytes());
        let mut expected = vec![0u8; vm::TABLE_BYTES as usize];
        if let Some(base) = backing {
            for n in 0..3 {
                let va = VA + n * 4096;
                if va / (128 << 20) != pdi {
                    continue;
                }
                let offset = ((va % (128 << 20)) / 4096 * 8) as usize;
                expected[offset..offset + 8]
                    .copy_from_slice(&(((base + n * 4096) >> 8) | bits).to_le_bytes());
            }
        }
        let actual = checked_bytes(io.p.access(), total, window, pointer, r)?;
        if actual != expected {
            return Err("VM complete small-page snapshot mismatch");
        }
        if !tables.is_empty() {
            tables.push(',');
            hashes.push(',');
        }
        let _ = write!(tables, "[{},{}]", pdi, r.start);
        let _ = write!(hashes, "\"{}\"", super::hash(&actual));
    }
    let actual = checked_bytes(io.p.access(), total, window, pointer, space.directory.range)?;
    if actual != directory {
        return Err("VM complete directory snapshot mismatch");
    }
    let mut ptes = Vec::new();
    for n in 0..5 {
        ptes.push(
            io.read(
                space
                    .pte_address(VA - 4096 + n * 4096)
                    .ok_or("VM boundary snapshot lacks a table")?,
            )?,
        );
    }
    Ok(format!("{{\"space\":{},\"phase\":\"{}\",\"instance\":{},\"directory\":{},\"tables\":[{}],\"binding\":[{},{}],\"directory_sha256\":\"{}\",\"tables_sha256\":[{}],\"ptes\":{:?}}}",
        index,phase,space.instance.range.start,space.directory.range.start,tables,space.directory.range.start,vm::LIMIT-1,super::hash(&actual),hashes,ptes))
}
fn guards(ranges: impl Iterator<Item = Range>) -> Vec<Range> {
    ranges
        .flat_map(|r| {
            [
                Range {
                    start: r.start - 4096,
                    size: 4096,
                },
                Range {
                    start: r.end().unwrap(),
                    size: 4096,
                },
            ]
        })
        .collect()
}
pub fn probe(
    regs: &mut Registers,
    device: &pci::Device,
    layout: &mut memory::Layout,
    console: &super::ConsoleSurface,
) -> Result<(), &'static str> {
    let baseline = layout.allocator.stats();
    let total = baseline.reserved + baseline.charged + baseline.free;
    let seed = (timer::rdtsc() as u32) | 1;
    let bar_before = regs.read(0x1704);
    let pramin_before = regs.read(0x1700);
    let surface = layout.objects[0].range;
    let (_, displacement, _) =
        modeset::bar1_view(surface.start, surface.size, device.bars[1].size, 17)?;
    let pointer = console.pointer - displacement;
    let mut io = DeviceIo {
        p: memory::Pramin::new(regs, total),
        invalidations: Vec::new(),
    };
    let (faults_before, before_records) = vm::faults(&mut io);
    crate::serial_println!(
        "GT710_VM_FAULT phase=before mask={} records=[{}]",
        faults_before,
        fault_json(&before_records)
    );
    if faults_before != 0 {
        return Err("preexisting GPU fault before private VM probe");
    }
    let mut protected = layout.allocator.reserved().to_vec();
    protected.extend(layout.objects.iter().map(memory::Object::guarded));
    crate::serial_println!("GT710_VM_STAGE reserved-before");
    let (protected_bytes, protected_before) = digest(io.p.access(), total, &protected)?;
    let result = (|| {
        let mut backing = Vec::new();
        for _ in 0..3 {
            backing.push(layout.allocator.allocate(12288, 4096, Kind::Probe)?);
        }
        let window = layout.allocator.allocate(65536, 4096, Kind::Engine)?;
        // This instance and both spaces are charged allocations; leave POST storage alone.
        for address in (window.range.start..window.range.end().unwrap()).step_by(8) {
            io.write(address, 0)?;
        }
        let mut a = Context::new(&mut layout.allocator, &mut io)?;
        let mut b = Context::new(&mut layout.allocator, &mut io)?;
        let initial = Flags {
            read_only: true,
            privileged: true,
            volatile: false,
        };
        a.map(&mut layout.allocator, &mut io, VA, backing[0], initial)?;
        b.map(
            &mut layout.allocator,
            &mut io,
            VA,
            backing[1],
            Flags::default(),
        )?;
        let peak = layout.allocator.stats();
        let owned = backing
            .iter()
            .map(|a| a.range)
            .chain(core::iter::once(window.range))
            .chain([
                a.space().instance.range,
                a.space().directory.range,
                b.space().instance.range,
                b.space().directory.range,
            ])
            .chain(a.space().table_ranges().map(|(_, r)| r))
            .chain(b.space().table_ranges().map(|(_, r)| r));
        let guard_ranges = guards(owned);
        let (guard_bytes, guard_before) = digest(io.p.access(), total, &guard_ranges)?;
        // Fill the three physical allocations before reading an independent view.
        for r in &backing {
            for address in (r.range.start..r.range.end().unwrap()).step_by(4) {
                io.p.write(address, pattern(address, seed, 0))?;
            }
        }
        io.flush()?;
        let mut backing_hashes = Vec::new();
        for r in &backing {
            let bytes = checked_bytes(io.p.access(), total, window.range, pointer, r.range)?;
            for (n, word) in bytes.chunks_exact(4).enumerate() {
                if word != pattern(r.range.start + n as u64 * 4, seed, 0).to_le_bytes() {
                    return Err("VM backing nonce mismatch");
                }
            }
            let h0 = super::hash(&bytes);
            for offset in (0..r.range.size).step_by(4) {
                // SAFETY: checked_bytes left this allocation's BAR view active.
                unsafe {
                    ((pointer + offset as usize) as *mut u32).write_volatile(pattern(
                        r.range.start + offset,
                        seed,
                        1,
                    ));
                }
            }
            // SAFETY: drain WC stores before the GPU BAR barrier/window change.
            unsafe {
                core::arch::asm!("sfence", options(nostack, preserves_flags));
            }
            io.flush()?;
            let bytes = io.p.bytes(r.range)?;
            for (n, word) in bytes.chunks_exact(4).enumerate() {
                if word != pattern(r.range.start + n as u64 * 4, seed, 1).to_le_bytes() {
                    return Err("VM reverse backing nonce mismatch");
                }
            }
            backing_hashes.push(format!("[\"{}\",\"{}\"]", h0, super::hash(&bytes)));
        }
        let mut snapshots = Vec::new();
        snapshots.push(snapshot(
            &mut io,
            total,
            window.range,
            pointer,
            a.space(),
            0,
            "map",
            Some(backing[0].range.start),
            7,
        )?);
        snapshots.push(snapshot(
            &mut io,
            total,
            window.range,
            pointer,
            b.space(),
            1,
            "map",
            Some(backing[1].range.start),
            1,
        )?);
        let snapshots_before = snapshots.clone();
        let tickets = [Consumer::Queued, Consumer::Readback, Consumer::Scanout]
            .into_iter()
            .map(|kind| a.hold(kind))
            .collect::<Result<Vec<_>, _>>()?;
        let mut stages = Vec::new();
        let stage = |phase: &str, context: &Context, allocator: &memory::allocator::Allocator| {
            format!(
                "{{\"phase\":\"{}\",\"holds\":{:?},\"stats\":{}}}",
                phase,
                context.holds(),
                stats(allocator.stats())
            )
        };
        stages.push(stage("held", &a, &layout.allocator));
        if layout.allocator.release(backing[0]).is_ok() {
            return Err("held backing recycled");
        }
        let mut table_blocks = 0;
        for allocation in a.space().allocations() {
            if layout.allocator.release(allocation).is_ok() {
                return Err("held VM table recycled");
            }
            table_blocks += 1;
        }
        if a.remap(&layout.allocator, &mut io, VA, backing[2], Flags::default())
            .is_ok()
            || a.unmap(&mut io, VA).is_ok()
            || a.retire(&mut layout.allocator, &mut io).is_ok()
        {
            return Err("consumer-held VM changed or retired");
        }
        a.fail_eio();
        if a.retire(&mut layout.allocator, &mut io).is_ok() {
            return Err("synthetic EIO recycled storage");
        }
        stages.push(stage("eio", &a, &layout.allocator));
        // Synthetic fault attribution exercises software ownership only. Do not
        // write PFIFO fault registers or imply an engine executed a translation.
        let instance = a.space().instance.range.start;
        let raw = [(instance >> 12) as u32, 0x1234, 0, 2];
        let fault = vm::Fault {
            unit: 5,
            raw,
            instance,
            address: 0x1234,
            reason: 2,
            client: 0,
            gpc: 0,
            hub: false,
            write: false,
        };
        let foreign = b.attribute(&fault);
        let own = a.attribute(&fault);
        let unknown = a.attribute(&vm::Fault {
            instance: 1 << 39,
            ..fault
        });
        if !own || foreign || unknown {
            return Err("VM fault attributed to wrong context");
        }
        let survivor = b.hold(Consumer::Readback)?;
        b.finish_cpu(survivor)?;
        a.reset_failed();
        if a.retire(&mut layout.allocator, &mut io).is_ok() {
            return Err("failed reset recycled storage");
        }
        stages.push(stage("reset-failed", &a, &layout.allocator));
        for (ticket, phase) in
            tickets
                .into_iter()
                .zip(["queued-done", "readback-done", "scanout-done"])
        {
            a.finish_cpu(ticket)?;
            if a.retire(&mut layout.allocator, &mut io).is_ok() {
                return Err("faulted consumer retirement recycled storage");
            }
            stages.push(stage(phase, &a, &layout.allocator));
        }
        let retained_snapshots = [
            snapshot(
                &mut io,
                total,
                window.range,
                pointer,
                a.space(),
                0,
                "map",
                Some(backing[0].range.start),
                7,
            )?,
            snapshot(
                &mut io,
                total,
                window.range,
                pointer,
                b.space(),
                1,
                "map",
                Some(backing[1].range.start),
                1,
            )?,
        ];
        if retained_snapshots.as_slice() != snapshots_before.as_slice()
            || layout.allocator.stats() != peak
        {
            return Err("fault containment changed storage or charges");
        }
        // Neither space has ever been admitted to a channel. CPU cancellation
        // may end this synthetic policy exercise; it cannot stand in for reset.
        a.cancel_unsubmitted()?;
        stages.push(stage("cpu-cancel", &a, &layout.allocator));
        let lifetime=format!("\"stages\":[{}],\"blocked\":{{\"release_backing\":1,\"release_tables\":{},\"remap\":1,\"unmap\":1,\"retire\":6}},\"fault\":{{\"raw\":{:?},\"instance\":{},\"address\":4660,\"reason\":2}},\"attributed\":[{},{},{}],\"unaffected\":true,\"snapshots_before\":[{}],\"snapshots_retained\":[{}]",
            stages.join(","),table_blocks,raw,instance,own,foreign,unknown,snapshots_before.join(","),retained_snapshots.join(","));
        a.remap(
            &layout.allocator,
            &mut io,
            VA,
            backing[2],
            Flags {
                volatile: true,
                ..Flags::default()
            },
        )?;
        snapshots.push(snapshot(
            &mut io,
            total,
            window.range,
            pointer,
            a.space(),
            0,
            "remap",
            Some(backing[2].range.start),
            (1 << 32) | 1,
        )?);
        a.unmap(&mut io, VA)?;
        snapshots.push(snapshot(
            &mut io,
            total,
            window.range,
            pointer,
            a.space(),
            0,
            "unmap",
            None,
            0,
        )?);
        a.map(&mut layout.allocator, &mut io, VA, backing[0], initial)?;
        snapshots.push(snapshot(
            &mut io,
            total,
            window.range,
            pointer,
            a.space(),
            0,
            "restore",
            Some(backing[0].range.start),
            7,
        )?);
        snapshots.push(snapshot(
            &mut io,
            total,
            window.range,
            pointer,
            b.space(),
            1,
            "unaffected",
            Some(backing[1].range.start),
            1,
        )?);
        // Revisit the three backing allocations after the table/window sequence to catch aliasing.
        for r in &backing {
            for address in (r.range.start..r.range.end().unwrap()).step_by(4) {
                if io.p.read(address)? != pattern(address, seed, 1) {
                    return Err("VM window change aliased backing");
                }
            }
        }
        let (_, guard_after) = digest(io.p.access(), total, &guard_ranges)?;
        if guard_before != guard_after {
            return Err("VM scratch guard changed");
        }
        a.retire(&mut layout.allocator, &mut io)?;
        b.retire(&mut layout.allocator, &mut io)?;
        // The outer cleanup restores BAR1 before releasing its private viewer.
        Ok((
            backing,
            window,
            peak,
            backing_hashes,
            snapshots,
            guard_bytes,
            guard_before,
            guard_after,
            lifetime,
        ))
    })();
    let display_directory = layout.objects[4].range.start + 0x3000;
    modeset::activate_bar1(
        io.p.access(),
        u64::from(bar_before & 0x0fffffff) << 12,
        display_directory,
    )?;
    if io.register(0x1704) != bar_before {
        return Err("VM probe failed to restore display BAR1");
    }
    let (
        backing,
        window,
        peak,
        backing_hashes,
        snapshots,
        guard_bytes,
        guard_before,
        guard_after,
        lifetime,
    ) = result?;
    crate::serial_println!("GT710_VM_STAGE reserved-after");
    let (_, protected_after) = digest(io.p.access(), total, &protected)?;
    if protected_before != protected_after {
        return Err("VM probe changed reserved storage");
    }
    let (faults_after, after_records) = vm::faults(&mut io);
    crate::serial_println!(
        "GT710_VM_FAULT phase=after mask={} records=[{}]",
        faults_after,
        fault_json(&after_records)
    );
    if faults_after != 0 {
        return Err("GPU fault during private VM construction");
    }
    let invalidations = format!("{:?}", io.invalidations);
    drop(io);
    for r in &backing {
        layout.allocator.release(*r)?;
    }
    layout.allocator.release(window)?;
    let after = layout.allocator.stats();
    if after != baseline {
        return Err("VM probe charge reclamation mismatch");
    }
    crate::serial_println!("GT710_LIFETIME {{\"schema\":1,\"bdf\":\"{:02x}:{:02x}.{}\",\"baseline\":{},\"peak\":{},\"after\":{},{},\"exercise\":\"cpu-only\",\"translation\":\"pending-m6\",\"reset_execution\":\"pending-m6\"}}",
        device.bus,device.device,device.function,stats(baseline),stats(peak),stats(after),lifetime);
    let ranges: Vec<[u64; 2]> = backing
        .iter()
        .map(|a| [a.range.start, a.range.size])
        .collect();
    crate::serial_println!("GT710_VM {{\"schema\":1,\"bdf\":\"{:02x}:{:02x}.{}\",\"seed\":{},\"va\":{},\"limit\":{},\"baseline\":{},\"peak\":{},\"after\":{},\"backing\":{:?},\"backing_sha256\":[{}],\"view\":[{},{}],\"snapshots\":[{}],\"invalidations\":{},\"protected_bytes\":{},\"protected_before\":\"{}\",\"protected_after\":\"{}\",\"guard_bytes\":{},\"guard_before\":\"{}\",\"guard_after\":\"{}\",\"bar_before\":{},\"bar_after\":{},\"pramin_before\":{},\"pramin_after\":{},\"faults_before\":{},\"faults_after\":{},\"fault_records_before\":[{}],\"fault_records_after\":[{}],\"translation\":\"pending-m6\"}}",
        device.bus,device.device,device.function,seed,VA,vm::LIMIT,stats(baseline),stats(peak),stats(after),ranges,backing_hashes.join(","),window.range.start,window.range.size,snapshots.join(","),invalidations,
        protected_bytes,protected_before,protected_after,guard_bytes,guard_before,guard_after,bar_before,regs.read(0x1704),pramin_before,regs.read(0x1700),faults_before,faults_after,fault_json(&before_records),fault_json(&after_records));
    Ok(())
}
