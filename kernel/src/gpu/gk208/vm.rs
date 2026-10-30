/*
 * Copyright 2017 Red Hat Inc.
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
 */
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
//! GK208B 40-bit VM, 128 KiB big-page geometry with 4 KiB SPTs only.
//! Encodings/attachment/invalidation: MIT nouveau v6.8 vmmgf100.c and
//! vmmgk104.c; fault fields: MIT engine/fifo/gf100.c (THIRD_PARTY_NOTICES).
//! These CPU-only builders are not channel admission. The caller owns backing
//! through retirement; lifetime.rs retains mappings/tables. Engine probes await M6.
use super::memory::{
    allocator::{Allocation, Allocator, Kind},
    Range,
};
use alloc::vec::Vec;

pub const LIMIT: u64 = 1 << 40;
pub const TABLE_BYTES: u64 = 32768 * 8;
const PAGE: u64 = 4096;
const COVERAGE: u64 = 1 << 27;

#[derive(Clone, Copy, Debug, Default)]
pub struct Flags {
    pub read_only: bool,
    pub privileged: bool,
    pub volatile: bool,
}
impl Flags {
    fn bits(self) -> u64 {
        1 | ((self.privileged as u64) << 1)
            | ((self.read_only as u64) << 2)
            | ((self.volatile as u64) << 32)
    }
}
/// Physical VRAM access plus device-visible publication and a bounded per-PDB
/// invalidate. Implementations must serialize this boot-time owner with BAR users.
pub trait Io {
    fn read(&mut self, address: u64) -> Result<u64, &'static str>;
    fn write(&mut self, address: u64, value: u64) -> Result<(), &'static str>;
    fn flush(&mut self) -> Result<(), &'static str>;
    fn invalidate(&mut self, directory: u64) -> Result<u32, &'static str>;
    fn register(&mut self, offset: usize) -> u32;
}

pub struct Space {
    pub instance: Allocation,
    pub directory: Allocation,
    tables: Vec<(u64, Allocation, u32)>,
    total: u64,
    poisoned: bool,
    detached: bool,
}
fn clear(io: &mut impl Io, r: Range) -> Result<(), &'static str> {
    for a in (r.start..r.end().ok_or("VM extent overflow")?).step_by(8) {
        io.write(a, 0)?;
    }
    Ok(())
}
impl Space {
    #[cfg(target_os="none")]
    pub(super) fn heap_vector_bytes(&self) -> usize {
        self.tables.capacity() * core::mem::size_of::<(u64, Allocation, u32)>()
    }
    pub fn new(a: &mut Allocator, io: &mut impl Io) -> Result<Self, &'static str> {
        let stats = a.stats();
        let total = stats.reserved + stats.charged + stats.free;
        let instance = a.allocate(PAGE, PAGE, Kind::Engine)?;
        let directory = match a.allocate(65536, PAGE, Kind::Engine) {
            Ok(v) => v,
            Err(e) => {
                a.release(instance)?;
                return Err(e);
            }
        };
        // A failed device operation retains the charged allocations. Do not
        // hand them to another owner on an uncertain hardware result.
        clear(io, instance.range)?;
        clear(io, directory.range)?;
        io.write(instance.range.start + 0x200, directory.range.start)?;
        io.write(instance.range.start + 0x208, LIMIT - 1)?;
        io.flush()?;
        io.invalidate(directory.range.start)?;
        Ok(Self {
            instance,
            directory,
            tables: Vec::new(),
            total,
            poisoned: false,
            detached: false,
        })
    }
    pub fn usable(&self) -> bool { !self.poisoned && !self.detached }
    pub fn allocations(&self) -> impl Iterator<Item = Allocation> + '_ {
        core::iter::once(self.instance)
            .chain(core::iter::once(self.directory))
            .chain(self.tables.iter().map(|(_, a, _)| *a))
    }
    pub fn table_ranges(&self) -> impl Iterator<Item = (u64, Range)> + '_ {
        self.tables.iter().map(|(i, a, _)| (*i, a.range))
    }
    pub fn pte_address(&self, va: u64) -> Option<u64> {
        if va >= LIMIT {
            return None;
        }
        self.tables
            .iter()
            .find(|(i, _, _)| *i == va / COVERAGE)
            .map(|(_, a, _)| a.range.start + ((va % COVERAGE) / PAGE) * 8)
    }
    fn validate(&self, va: u64, backing: Range) -> Result<(), &'static str> {
        if self.poisoned || self.detached {
            return Err("VM retained after uncertain device operation");
        }
        if va % PAGE != 0
            || backing.start % PAGE != 0
            || backing.size == 0
            || backing.size % PAGE != 0
            || va.checked_add(backing.size).is_none_or(|e| e > LIMIT)
            || backing.end().is_none_or(|e| e > self.total)
        {
            return Err("invalid VM mapping extent");
        }
        Ok(())
    }
    fn present(
        &self,
        io: &mut impl Io,
        va: u64,
        size: u64,
        wanted: bool,
    ) -> Result<(), &'static str> {
        for v in (va..va + size).step_by(PAGE as usize) {
            let present = match self.pte_address(v) {
                Some(a) => io.read(a)? != 0,
                None => false,
            };
            if present != wanted {
                return Err("VM mapping presence mismatch");
            }
        }
        Ok(())
    }
    fn publish(&mut self, io: &mut impl Io) -> Result<(), &'static str> {
        io.flush()?;
        io.invalidate(self.directory.range.start)?;
        Ok(())
    }
    /// Caller holds the backing allocation for the entire mapping lifetime.
    pub fn map(
        &mut self,
        a: &mut Allocator,
        io: &mut impl Io,
        va: u64,
        backing: Range,
        flags: Flags,
    ) -> Result<(), &'static str> {
        self.map_kind(a, io, va, backing, flags, 0)
    }
    /// Uncompressed storage kind, carried in GF100-family PTE bits 36..43
    /// (MIT nouveau v6.8 vmmgf100.c:319). Existing linear callers use kind 0.
    pub fn map_kind(
        &mut self, a: &mut Allocator, io: &mut impl Io, va: u64,
        backing: Range, flags: Flags, kind: u8,
    ) -> Result<(), &'static str> {
        self.validate(va, backing)?;
        self.present(io, va, backing.size, false)?;
        let result = (|| {
            let last = (va + backing.size - 1) / COVERAGE;
            for index in va / COVERAGE..=last {
                if self.tables.iter().any(|(i, _, _)| *i == index) {
                    continue;
                }
                self.tables
                    .try_reserve(1)
                    .map_err(|_| "VM table metadata exhausted")?;
                let table = a.allocate(TABLE_BYTES, PAGE, Kind::Engine)?;
                clear(io, table.range)?;
                // Empty table must be visible before publishing its SPT PDE.
                io.flush()?;
                io.write(
                    self.directory.range.start + index * 8,
                    ((table.range.start >> 8) | 1) << 32,
                )?;
                self.tables.push((index, table, 0));
            }
            self.write_mapping(io, va, backing, flags, kind)?;
            self.account_pages(va, backing.size, true);
            self.publish(io)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    /// Map caller-retained coherent DMA pages. The caller must keep the physical
    /// pages pinned until engine drain and must validate platform DMA translation.
    pub fn map_system(
        &mut self, a: &mut Allocator, io: &mut impl Io, va: u64, pages: &[u64],
    ) -> Result<(), &'static str> {
        if pages.is_empty() || pages.len() > 256
            || pages.iter().any(|p| p % PAGE != 0 || *p >= LIMIT)
        { return Err("invalid coherent DMA pages"); }
        let size = pages.len() as u64 * PAGE;
        // Allocate/clear SPTs through the regular bounded constructor. No channel
        // is admitted while these temporary VRAM entries are being replaced.
        self.map(a, io, va, Range { start: 0, size }, Flags::default())?;
        let result = (|| {
            for (i, p) in pages.iter().enumerate() {
                io.write(self.pte_address(va + i as u64 * PAGE).ok_or("missing system PTE")?,
                    (p >> 8) | 1 | (1 << 32) | (2 << 33))?;
            }
            self.publish(io)
        })();
        if result.is_err() { self.poisoned = true; }
        result
    }
    fn write_mapping(
        &self,
        io: &mut impl Io,
        va: u64,
        backing: Range,
        flags: Flags,
        kind: u8,
    ) -> Result<(), &'static str> {
        for offset in (0..backing.size).step_by(PAGE as usize) {
            io.write(
                self.pte_address(va + offset)
                    .ok_or("missing VM page table")?,
                ((backing.start + offset) >> 8) | flags.bits() | (u64::from(kind) << 36),
            )?;
        }
        Ok(())
    }
    pub fn remap(
        &mut self,
        io: &mut impl Io,
        va: u64,
        backing: Range,
        flags: Flags,
    ) -> Result<(), &'static str> {
        self.validate(va, backing)?;
        self.present(io, va, backing.size, true)?;
        let result = self
            .write_mapping(io, va, backing, flags, 0)
            .and_then(|()| self.publish(io));
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    pub fn unmap(&mut self, io: &mut impl Io, va: u64, size: u64) -> Result<(), &'static str> {
        self.validate(va, Range { start: 0, size })?;
        self.present(io, va, size, true)?;
        let result = (|| {
            for v in (va..va + size).step_by(PAGE as usize) {
                io.write(self.pte_address(v).ok_or("missing VM page table")?, 0)?;
            }
            self.publish(io)?;
            self.account_pages(va, size, false);
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    fn account_pages(&mut self, va: u64, size: u64, add: bool) {
        for (index, _, pages) in &mut self.tables {
            let start = va.max(*index * COVERAGE);
            let end = (va + size).min((*index + 1) * COVERAGE);
            if start < end {
                let count = ((end - start) / PAGE) as u32;
                if add { *pages += count; } else { *pages -= count; }
            }
        }
    }
    /// Remove empty SPTs from this PDB and its channel alias before recycling.
    /// An uncertain directory publication retains the table and its charge.
    pub(super) fn reclaim_empty(&mut self, a: &mut Allocator, io: &mut impl Io,
        alias: Option<u64>,
    ) -> Result<Vec<u64>, &'static str> {
        if !self.usable() { return Err("VM retained after uncertain device operation"); }
        let mut empty = Vec::new();
        empty.try_reserve_exact(self.tables.len()).map_err(|_| "VM reclamation scratch allocation")?;
        empty.extend(self.tables.iter().filter(|(_, _, pages)| *pages == 0)
            .map(|(index, _, _)| *index));
        if empty.is_empty() { return Ok(empty); }
        let result = (|| {
            for &index in &empty {
                io.write(self.directory.range.start + index * 8, 0)?;
                if let Some(alias) = alias { io.write(alias + index * 8, 0)?; }
            }
            self.publish(io)?;
            if let Some(alias) = alias { io.invalidate(alias)?; }
            Ok(())
        })();
        if let Err(error) = result {
            self.poisoned = true;
            return Err(error);
        }
        for &index in &empty {
            let at = self.tables.iter().position(|(i, _, _)| *i == index).unwrap();
            a.release(self.tables[at].1)?;
            self.tables.remove(at);
        }
        Ok(empty)
    }
    /// Detach while storage is still pinned. A failed invalidate retains it.
    pub(super) fn detach(&mut self, io: &mut impl Io) -> Result<(), &'static str> {
        if self.poisoned {
            return Err("VM retained after uncertain device operation");
        }
        if self.detached {
            return Ok(());
        }
        let result = (|| {
            io.write(self.instance.range.start + 0x200, 0)?;
            io.write(self.instance.range.start + 0x208, 0)?;
            io.flush()?;
            io.invalidate(self.directory.range.start)?;
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        } else {
            self.detached = true;
        }
        result
    }
    pub(super) fn reclaim(self, a: &mut Allocator) -> Result<(), &'static str> {
        if !self.detached {
            return Err("VM still attached");
        }
        for (_, table, _) in self.tables {
            a.release(table)?;
        }
        a.release(self.directory)?;
        a.release(self.instance)
    }
    /// Only for CPU-only spaces not exposed to a channel.
    /// Engine owners use lifetime::Context and its verified drain/reset contract.
    #[allow(dead_code)] // Public CPU-only builder API; engine owners use Context.
    pub fn retire(mut self, a: &mut Allocator, io: &mut impl Io) -> Result<(), &'static str> {
        self.detach(io)?;
        self.reclaim(a)
    }
    /// # Safety
    /// The caller must have confirmed that device reset ended DMA for this space.
    pub(super) unsafe fn reset_confirmed(&mut self) {
        self.poisoned = false;
        self.detached = false;
    }
}
#[derive(Debug)]
pub struct Fault {
    pub unit: u8,
    pub raw: [u32; 4],
    pub instance: u64,
    pub address: u64,
    pub reason: u8,
    pub client: u8,
    pub gpc: u8,
    pub hub: bool,
    pub write: bool,
}
pub fn faults(io: &mut impl Io) -> (u32, Vec<Fault>) {
    let mask = io.register(0x259c);
    let mut records = Vec::new();
    for unit in 0..32 {
        if mask & (1 << unit) == 0 {
            continue;
        }
        let r = 0x2800 + unit * 16;
        let raw = core::array::from_fn(|i| io.register(r + i * 4));
        let bits = raw[3];
        records.push(Fault {
            unit: unit as u8,
            raw,
            instance: u64::from(raw[0]) << 12,
            address: u64::from(raw[1]) | (u64::from(raw[2]) << 32),
            reason: (bits & 15) as u8,
            client: ((bits >> 8) & 31) as u8,
            gpc: ((bits >> 24) & 31) as u8,
            hub: bits & 64 != 0,
            write: bits & 128 != 0,
        });
    }
    (mask, records)
}
