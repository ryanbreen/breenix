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
//! M5.3 CPU apertures and coherent DMA preparation; engine execution awaits M6.
use super::{
    aperture, dma_policy,
    inventory::{digest, pattern, stats, words},
    memory::{self, allocator::Kind, Range},
    modeset, system_dma, Registers,
};
use crate::{
    arch_impl::x86_64::{pat, timer},
    drivers::pci,
};
use alloc::{format, string::String, vec::Vec};
impl aperture::Io for Registers {
    fn drain_wc(&mut self) {
        crate::gpu_cache::drain_wc();
    }
}

/// Inspect the active CPU leaf, including the large-page PAT bit location.
fn memory_type(va: u64) -> Result<u64, &'static str> {
    let mut table = x86_64::registers::control::Cr3::read()
        .0
        .start_address()
        .as_u64();
    let offset = crate::memory::physical_memory_offset().as_u64();
    for level in (1..=4).rev() {
        let index = ((va >> (12 + 9 * (level - 1))) & 511) as usize;
        // SAFETY: active page tables are in the physical map, read-only here.
        let entry = unsafe { ((offset + table) as *const u64).add(index).read_volatile() };
        if entry & 1 == 0 {
            return Err("CPU aperture is not mapped");
        }
        if level == 1 || (level <= 3 && entry & (1 << 7) != 0) {
            let bit = if level == 1 { 7 } else { 12 };
            return Ok(((entry >> 3) & 3) | (((entry >> bit) & 1) << 2));
        }
        table = entry & 0x000f_ffff_ffff_f000;
    }
    Err("CPU aperture has no leaf")
}
fn checked_type(pointer: u64, size: u64, expected: u64) -> Result<u64, &'static str> {
    for at in (pointer..pointer.checked_add(size).ok_or("CPU mapping overflow")?).step_by(4096) {
        if memory_type(at)? != expected {
            return Err("CPU mapping has the wrong PAT index");
        }
    }
    Ok(expected)
}
fn view(
    regs: &mut Registers,
    total: u64,
    bar: u8,
    table: Range,
    r: Range,
) -> Result<String, &'static str> {
    let (pde, ptes) = modeset::bar1_small_pages(r.start, r.size, table.start + 8192)?;
    words(
        regs,
        total,
        table.start + 0x200,
        &[
            (table.start + 4096) as u32,
            0,
            (modeset::BAR1_WINDOW - 1) as u32,
            0,
        ],
    )?;
    words(regs, total, table.start + 4096, &pde)?;
    let mut p = memory::Pramin::new(regs, total);
    for (i, value) in ptes.iter().enumerate() {
        p.write(table.start + 8192 + i as u64 * 8, *value as u32)?;
        p.write(table.start + 8196 + i as u64 * 8, (*value >> 32) as u32)?;
    }
    let binding = p.bytes(Range {
        start: table.start + 0x200,
        size: 16,
    })?;
    let expected = [table.start + 4096, (modeset::BAR1_WINDOW - 1) as u64];
    if binding
        != expected
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>()
    {
        return Err("CPU BAR instance readback differs");
    }
    if p.bytes(Range {
        start: table.start + 4096,
        size: 8,
    })? != pde.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>()
    {
        return Err("CPU BAR PDE readback differs");
    }
    let bytes = p.bytes(Range {
        start: table.start + 8192,
        size: ptes.len() as u64 * 8,
    })?;
    if bytes
        != ptes
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>()
    {
        return Err("CPU BAR SPT readback differs");
    }
    drop(p);
    aperture::activate(regs, bar, table.start, table.start + 4096)?;
    Ok(super::hash(&bytes))
}
fn fault(regs: &mut Registers) -> (u32, String) {
    let mask = regs.read(0x259c);
    let mut records = Vec::new();
    for i in 0..32 {
        if mask & (1 << i) != 0 {
            records.push([
                i as u32,
                regs.read(0x2800 + i * 16),
                regs.read(0x2804 + i * 16),
                regs.read(0x2808 + i * 16),
                regs.read(0x280c + i * 16),
            ]);
        }
    }
    (mask, format!("{:?}", records))
}
pub fn probe(
    regs: &mut Registers,
    device: &pci::Device,
    layout: &mut memory::Layout,
    console: &super::ConsoleSurface,
) -> Result<(), &'static str> {
    let statuses = system_dma::translation_status()?;
    let pages = system_dma::Pages::new(3, &statuses)?;
    let seed = (timer::rdtsc() as u32) | 1;
    let mut types = Vec::new();
    if !pat::supported() || pat::programmed_mask() & 1 == 0 {
        return Err("CPU PAT unavailable for GPU apertures");
    }
    // BAR0 is UC. BAR1 reuses the display WC mapping; BAR2 gets one WC view.
    types.push(checked_type(regs.0 as u64, 0x800000, 3)?);
    let surface = layout.objects[0].range;
    let (_, displacement, _) =
        modeset::bar1_view(surface.start, surface.size, device.bars[1].size, 17)?;
    let bar1 = console.pointer - displacement;
    types.push(checked_type(bar1 as u64, modeset::BAR1_WINDOW as u64, 1)?);
    let bar = device.bars[3]; // BAR1 is 64-bit; the second memory aperture is PCI BAR3.
    if bar.address == 0 || bar.is_io || !bar.prefetchable || bar.size < modeset::BAR1_WINDOW as u64
    {
        return Err("BAR2 CPU aperture is unavailable");
    }
    let bar2 = crate::memory::map_mmio(bar.address, modeset::BAR1_WINDOW)?;
    crate::console::display::linear::map_native_write_combining(bar.address, bar.size)?;
    types.push(checked_type(bar2 as u64, modeset::BAR1_WINDOW as u64, 1)?);
    let mut dma = Vec::new();
    for i in 0..3 {
        types.push(checked_type(pages.va(i), 4096, 0)?);
        for offset in (0..4096).step_by(4) {
            // SAFETY: owned WB page, complete aligned words.
            unsafe {
                ((pages.va(i) + offset) as *mut u32).write_volatile(pattern(
                    pages.address(i) + offset,
                    seed,
                    3,
                ));
            }
        }
    }
    pages.publish();
    pages.observe();
    for i in 0..3 {
        // SAFETY: these frames have not been handed to an engine.
        let bytes = unsafe { core::slice::from_raw_parts(pages.va(i) as *const u8, 4096) };
        for (n, word) in bytes.chunks_exact(4).enumerate() {
            if word != pattern(pages.address(i) + n as u64 * 4, seed, 3).to_le_bytes() {
                return Err("coherent system page CPU readback differs");
            }
        }
        dma.push(format!(
            "[{},{},\"{}\"]",
            pages.address(i),
            dma_policy::coherent_pte(pages.address(i))?,
            super::hash(bytes)
        ));
    }
    let baseline = layout.allocator.stats();
    let total = baseline.reserved + baseline.charged + baseline.free;
    let before = [regs.read(0x1704), regs.read(0x1714)];
    let pramin = regs.read(0x1700);
    let (faults_before, records_before) = fault(regs);
    if faults_before != 0 {
        return Err("pending fault before CPU aperture test");
    }
    let mut protected = layout.allocator.reserved().to_vec();
    protected.extend(layout.objects.iter().map(memory::Object::guarded));
    crate::serial_println!("GT710_APERTURE_STAGE reserved-before");
    let (protected_bytes, protected_before) = digest(regs, total, &protected)?;
    let a = layout
        .allocator
        .allocate((1 << 20) + 4096, 4096, Kind::Probe)?;
    let b = layout.allocator.allocate(12288, 4096, Kind::Probe)?;
    let tables = [
        layout.allocator.allocate(65536, 4096, Kind::Engine)?,
        layout.allocator.allocate(65536, 4096, Kind::Engine)?,
    ];
    let peak = layout.allocator.stats();
    let ranges = [a.range, b.range];
    let mut guards = Vec::new();
    for allocation in [a, b, tables[0], tables[1]] {
        guards.extend([
            Range {
                start: allocation.range.start - 4096,
                size: 4096,
            },
            Range {
                start: allocation.range.end().unwrap(),
                size: 4096,
            },
        ]);
    }
    let (guard_bytes, guard_before) = digest(regs, total, &guards)?;
    // Clear new owned table payloads. Inherited storage stays read-only.
    {
        let mut p = memory::Pramin::new(regs, total);
        for table in &tables {
            for address in (table.range.start..table.range.end().unwrap()).step_by(4) {
                p.write(address, 0)?;
            }
        }
    }
    // Construct coherent system-memory PTEs in owned VRAM, without attaching
    // them to a channel. Read these exact entries back through both CPU views.
    for i in 0..3 {
        let pte = dma_policy::coherent_pte(pages.address(i))?;
        words(
            regs,
            total,
            tables[1].range.start + 0x8000 + i as u64 * 8,
            &[pte as u32, (pte >> 32) as u32],
        )?;
    }
    let mut views = Vec::new();
    let result = (|| {
        for (bar, pointer) in [(1u8, bar1), (2, bar2)] {
            // Fill both allocations before changing windows; distinct physical
            // address input detects accidental aliases across allocations/banks.
            {
                let mut p = memory::Pramin::new(regs, total);
                for r in ranges {
                    for address in (r.start..r.end().unwrap()).step_by(4) {
                        p.write(address, pattern(address, seed, 0))?;
                    }
                }
            }
            for index in [0, 1, 0] {
                let r = ranges[index];
                let table = tables[bar as usize - 1].range;
                let table_hash = view(regs, total, bar, table, r)?;
                let mut hashes = Vec::new();
                for direction in [0, bar as usize] {
                    if direction != 0 {
                        for offset in (0..r.size).step_by(4) {
                            // SAFETY: the bounded WC aperture was installed above.
                            unsafe {
                                ((pointer + offset as usize) as *mut u32).write_volatile(pattern(
                                    r.start + offset,
                                    seed,
                                    direction,
                                ));
                            }
                        }
                        crate::gpu_cache::drain_wc();
                        modeset::bar_flush(regs)?;
                    }
                    let mut hash = crate::block::install::sha256::Sha256::new();
                    let mut p = memory::Pramin::new(regs, total);
                    for offset in (0..r.size).step_by(4) {
                        let actual = if direction == 0 {
                            // SAFETY: bounded aligned read from the active BAR.
                            unsafe { ((pointer + offset as usize) as *const u32).read_volatile() }
                        } else {
                            p.read(r.start + offset)?
                        };
                        if actual != pattern(r.start + offset, seed, direction) {
                            return Err("CPU BAR/PRAMIN nonce mismatch");
                        }
                        hash.update(&actual.to_le_bytes());
                    }
                    hashes.push(super::hex(&hash.finalize()));
                    // A is revisited in A-B-A; reset it to the first direction.
                    if direction != 0 && index == 0 {
                        for offset in (0..r.size).step_by(4) {
                            p.write(r.start + offset, pattern(r.start + offset, seed, 0))?;
                        }
                    }
                }
                views.push(format!("{{\"bar\":{},\"index\":{},\"instance\":{},\"directory\":{},\"table\":{},\"binding\":[{},{}],\"pde\":[0,{}],\"ptes_sha256\":\"{}\",\"sha256\":[\"{}\",\"{}\"]}}",bar,index,table.start,table.start+4096,table.start+8192,table.start+4096,modeset::BAR1_WINDOW-1,((table.start+8192)>>8)|1,table_hash,hashes[0],hashes[1]));
            }
            let mut p = memory::Pramin::new(regs, total);
            for (index, r) in ranges.into_iter().enumerate() {
                for address in (r.start..r.end().unwrap()).step_by(4) {
                    if p.read(address)?
                        != pattern(address, seed, if index == 0 { 0 } else { bar as usize })
                    {
                        return Err("CPU BAR window aliased another allocation");
                    }
                }
            }
        }
        view(regs, total, 1, tables[0].range, tables[1].range)?;
        let mut p = memory::Pramin::new(regs, total);
        for i in 0..3 {
            let at = 0x8000 + i * 8;
            let expected = dma_policy::coherent_pte(pages.address(i))?;
            let physical = u64::from(p.read(tables[1].range.start + at as u64)?)
                | (u64::from(p.read(tables[1].range.start + at as u64 + 4)?) << 32);
            // SAFETY: BAR1 now views the owned BAR2 table payload.
            let bar = unsafe { ((bar1 + at) as *const u64).read_volatile() };
            if physical != expected || bar != expected {
                return Err("coherent DMA PTE independent readback differs");
            }
        }
        Ok(())
    })();
    // Restore controls before releasing tables. No inherited storage is written.
    crate::gpu_cache::drain_wc();
    modeset::bar_flush(regs)?;
    for (index, control) in [(0, 0x1704), (1, 0x1714)] {
        regs.write(control, before[index]);
        modeset::bar1_wait(regs)?;
        // Invalidate the outgoing private view even if the inherited VM is disabled.
        super::vm_control::invalidate(regs, tables[index].range.start + 4096)?;
    }
    if [regs.read(0x1704), regs.read(0x1714)] != before {
        return Err("CPU BAR control restore failed");
    }
    // Restore display translations through the already-owned directory as well.
    modeset::activate_bar1(
        regs,
        u64::from(before[0] & 0x0fffffff) << 12,
        layout.objects[4].range.start + 0x3000,
    )?;
    result?;
    crate::serial_println!("GT710_APERTURE_STAGE reserved-after");
    let (_, protected_after) = digest(regs, total, &protected)?;
    let (_, guard_after) = digest(regs, total, &guards)?;
    if protected_before != protected_after || guard_before != guard_after {
        return Err("CPU aperture changed protected VRAM");
    }
    let (faults_after, records_after) = fault(regs);
    if faults_after != 0 {
        return Err("GPU fault during CPU aperture test");
    }
    for allocation in [a, b, tables[0], tables[1]] {
        layout.allocator.release(allocation)?;
    }
    let after = layout.allocator.stats();
    if after != baseline {
        return Err("CPU aperture charge reclamation differs");
    }
    crate::serial_println!("GT710_APERTURE {{\"schema\":1,\"bdf\":\"{:02x}:{:02x}.{}\",\"seed\":{},\"baseline\":{},\"peak\":{},\"after\":{},\"scratch\":[[{},{}],[{},{}]],\"tables\":[[{},{}],[{},{}]],\"views\":[{}],\"dma\":[{}],\"dma_bits\":40,\"dma_access\":\"pending-m6\",\"translation\":\"pending-m6\",\"dmar_status\":{:?},\"memory_types\":{:?},\"bar_before\":{:?},\"bar_after\":{:?},\"pramin_before\":{},\"pramin_after\":{},\"protected_bytes\":{},\"protected_before\":\"{}\",\"protected_after\":\"{}\",\"guard_bytes\":{},\"guard_before\":\"{}\",\"guard_after\":\"{}\",\"faults_before\":{},\"faults_after\":{},\"fault_records_before\":{},\"fault_records_after\":{}}}",
        device.bus,device.device,device.function,seed,stats(baseline),stats(peak),stats(after),a.range.start,a.range.size,b.range.start,b.range.size,tables[0].range.start,tables[0].range.size,tables[1].range.start,tables[1].range.size,views.join(","),dma.join(","),statuses,types,before,[regs.read(0x1704),regs.read(0x1714)],pramin,regs.read(0x1700),protected_bytes,protected_before,protected_after,guard_bytes,guard_before,guard_after,faults_before,faults_after,records_before,records_after);
    Ok(())
}
