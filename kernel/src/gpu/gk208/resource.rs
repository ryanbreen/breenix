/*
 * Copyright 2008 Ben Skeggs
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
 * THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 */

//! Native resource storage. Layout sizing adapts Mesa 24.3.4 nvc0_miptree.c
//! and nv50_miptree.c (MIT); see THIRD_PARTY_NOTICES. Breenix owns allocation,
//! mappings, pinned guest backing and retirement. No pixels are produced here.
use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use super::{memory::allocator::{Allocation, Allocator, Kind, Pin}, vm::{self, Io, Space}};

pub const GOB_WIDTH: u32 = 64;
pub const GOB_HEIGHT: u32 = 8;
pub const GOB_BYTES: u64 = 512;
const MAX_DIMENSION: u32 = 16384;
const FIRST_VA: u64 = 0x1_0000_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pub texel_bytes: u32,
    pub pitch: u32,
    pub block_height: u32,
    pub tile_mode: u32,
    pub bytes: u64,
}
impl Layout {
    pub fn new(width: u32, height: u32, texel_bytes: u32) -> Result<Self, &'static str> {
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION
            || !matches!(texel_bytes, 1 | 4) { return Err("invalid native image extent"); }
        // Mesa nv50_tex_choose_tile_dims_helper, non-3D path. Kepler GOBs
        // are 64 bytes by 8 rows; a block contains 1, 2, 4, 8 or 16 GOBs.
        let shift = if height > 64 { 4 } else if height > 32 { 3 }
            else if height > 16 { 2 } else if height > 8 { 1 } else { 0 };
        let block_height = 1 << shift;
        let pitch = (width * texel_bytes).next_multiple_of(GOB_WIDTH);
        let rows = height.next_multiple_of(GOB_HEIGHT * block_height);
        Ok(Self { width, height, texel_bytes, pitch, block_height,
            tile_mode: shift << 4, bytes: u64::from(pitch) * u64::from(rows) })
    }
    pub fn block_bytes(self) -> u64 { GOB_BYTES * u64::from(self.block_height) }
    pub fn allocation_bytes(self) -> u64 { self.bytes.next_multiple_of(4096) }
    /// Byte address inside the uncompressed 2D block-linear surface. Kepler's
    /// GOB interleaves 16-byte sectors in pairs of rows, then 32-byte columns.
    pub fn offset(self, x: u32, y: u32, channel: u32) -> Option<u64> {
        if x >= self.width || y >= self.height || channel >= self.texel_bytes { return None; }
        let xb = x * self.texel_bytes + channel;
        let block_rows = GOB_HEIGHT * self.block_height;
        let block_row = u64::from(y / block_rows) * u64::from(self.pitch) * u64::from(block_rows);
        let block_column = u64::from(xb / GOB_WIDTH) * self.block_bytes();
        let gob = u64::from((y % block_rows) / GOB_HEIGHT) * GOB_BYTES;
        let sector = (xb % 64 / 32) * 256 + (y % 8 / 2) * 64
            + (xb % 32 / 16) * 32 + (y % 2) * 16 + xb % 16;
        Some(block_row + block_column + gob + u64::from(sector))
    }
}

/// Cloned by queued work, readback or scanout owners. An unref cannot unmap
/// or recycle anything while one of these tokens survives.
pub struct Hold { _reference: Arc<()> }
struct Entry<B> {
    layout: Option<Layout>,
    bytes: u64,
    va: u64,
    allocation: Option<Allocation>,
    pin: Option<Pin>,
    backing: Option<Arc<B>>,
    mapped: bool,
    dropped: bool,
    reference: Arc<()>,
    backing_va: Option<u64>,
    backing_bytes: u64,
    backing_mapped: bool,
}
/// One native storage address space; channel attachment may retain aliases in
/// the channel's private space. No userspace address or method is accepted.
pub struct Resources<B> {
    entries: BTreeMap<u32, Entry<B>>,
    space: Option<Space>,
    next_va: u64,
    vram_bytes: u64,
    alias: Option<u64>,
    channel: u64,
    incarnation: Option<(u64,u64)>,
    alias_indices: Vec<u64>,
    temporary: u32,
}
impl<B> Default for Resources<B> { fn default() -> Self { Self::new() } }
impl<B> Resources<B> {
    pub const fn new() -> Self {
        Self { entries: BTreeMap::new(), space: None, next_va: FIRST_VA, vram_bytes: 0, alias: None, channel: 0, incarnation: None, alias_indices: Vec::new(), temporary: u32::MAX }
    }
    pub fn vram_bytes(&self) -> u64 { self.vram_bytes }
    pub fn layout(&self, id: u32) -> Option<Layout> { self.entries.get(&id)?.layout }
    pub fn address(&self, id: u32) -> Option<u64> { Some(self.entries.get(&id)?.va) }
    pub fn pte_address(&self, va: u64) -> Option<u64> { self.space.as_ref()?.pte_address(va) }
    fn account(&mut self, before: u64, after: u64) {
        if after >= before { self.vram_bytes += after - before; }
        else { self.vram_bytes -= before - after; }
    }
    fn space(&mut self, a: &mut Allocator, io: &mut impl Io) -> Result<&mut Space, &'static str> {
        if self.space.is_none() { self.space = Some(Space::new(a, io)?); }
        Ok(self.space.as_mut().unwrap())
    }
    pub fn create(&mut self, a: &mut Allocator, io: &mut impl Io, id: u32,
        kind: u32, format: u32, width: u32, height: u32,
    ) -> Result<(), &'static str> {
        if id == 0 || self.entries.contains_key(&id) { return Err("duplicate native resource"); }
        let layout = match (kind, format) {
            (1 | 2, 0 | 1) => Some(Layout::new(width, height, 4)?),
            (2, 2) => Some(Layout::new(width, height, 1)?),
            (3, 0) if width != 0 && width <= 128 << 20 && height == 1 => None,
            _ => return Err("unsupported native resource format"),
        };
        let bytes = layout.map_or(u64::from(width), Layout::allocation_bytes);
        let va = self.next_va;
        let end = va.checked_add(bytes.next_multiple_of(4096)).filter(|v| *v <= vm::LIMIT)
            .ok_or("native resource VA exhausted")?;
        self.next_va = end;
        let before = a.stats().charged;
        let result = (|| {
            // Insert before publication so even a partial failed mapping keeps
            // its allocation, backing and charge reachable for retention.
            let allocation = if layout.is_some() { Some(a.allocate(bytes, 4096, Kind::Engine)?) } else { None };
            let pin = allocation.map(|v| a.pin(v)).transpose()?;
            self.entries.insert(id, Entry { layout, bytes, va, allocation, pin,
                backing: None, mapped: false, dropped: false, reference: Arc::new(()), backing_va: None, backing_bytes: 0, backing_mapped: false });
            if let Some(allocation) = allocation {
                self.space(a, io)?.map_kind(a, io, va, allocation.range, vm::Flags::default(), 0xfe)?;
                self.entries.get_mut(&id).unwrap().mapped = true;
            }
            Ok(())
        })();
        self.account(before, a.stats().charged);
        result
    }
    pub fn attach(&mut self, a: &mut Allocator, io: &mut impl Io, id: u32,
        backing: Arc<B>, pages: &[u64], bytes: u64,
    ) -> Result<(), &'static str> {
        let entry = self.entries.get(&id).ok_or("native resource absent")?;
        let required = entry.layout.map_or(entry.bytes, |l| u64::from(l.pitch) * u64::from(l.height));
        if entry.dropped || entry.backing.is_some() || bytes < required {
            return Err("invalid native resource backing");
        }
        let buffer = entry.layout.is_none(); let va = entry.va;
        if buffer && (pages.len() as u64 * 4096 != entry.bytes.next_multiple_of(4096)
            || pages.iter().any(|p| *p % 4096 != 0 || *p >= vm::LIMIT)) {
            return Err("invalid pinned native buffer");
        }
        self.entries.get_mut(&id).unwrap().backing = Some(backing);
        let before = a.stats().charged;
        let result = (|| {
            if buffer {
                // The existing VM builder bounds each publication to 256 pages.
                // Keep the full backing pinned before publishing the first band.
                for (i, band) in pages.chunks(256).enumerate() {
                    self.space(a, io)?.map_system(a, io, va + i as u64 * 256 * 4096, band)?;
                }
                self.entries.get_mut(&id).unwrap().mapped = true;
            }
            Ok(())
        })();
        self.account(before, a.stats().charged);
        result
    }
    pub fn hold(&self, id: u32) -> Result<Hold, &'static str> {
        let e = self.entries.get(&id).filter(|e| !e.dropped).ok_or("native resource absent")?;
        Ok(Hold { _reference: e.reference.clone() })
    }
    /// Map the attached, caller-retained backing once, separately from VRAM.
    pub fn map_backing(&mut self, a: &mut Allocator, io: &mut impl Io, id: u32,
        backing: &Arc<B>, pages: &[u64], bytes: u64,
    ) -> Result<u64, &'static str> {
        let e = self.entries.get(&id).filter(|e| !e.dropped).ok_or("native resource absent")?;
        if !e.backing.as_ref().is_some_and(|b| Arc::ptr_eq(b,backing))
            || pages.len() as u64 * 4096 != bytes || pages.is_empty() {
            return Err("native transfer backing mismatch");
        }
        if e.layout.is_none() { return Ok(e.va); }
        if let Some(va) = e.backing_va {
            return if e.backing_mapped { Ok(va) } else { Err("native backing mapping uncertain") };
        }
        let va = self.next_va;
        self.next_va = va.checked_add(bytes).filter(|e| *e <= vm::LIMIT).ok_or("native backing VA exhausted")?;
        // Save the mapping address before PTE publication. A publication failure
        // leaves the backing and poisoned Space in this resource entry.
        self.entries.get_mut(&id).unwrap().backing_va = Some(va);
        self.entries.get_mut(&id).unwrap().backing_bytes = bytes;
        let before = a.stats().charged;
        let result = (|| {
            for (i,band) in pages.chunks(256).enumerate() {
                self.space(a,io)?.map_system(a,io,va+i as u64*256*4096,band)?;
            }
            self.entries.get_mut(&id).unwrap().backing_mapped = true;
            Ok(va)
        })();
        self.account(before,a.stats().charged);
        result
    }
    pub fn channel(&self) -> u64 { self.channel }
    pub fn identity(&self, id: u32) -> Option<u64> {
        self.entries.get(&id).filter(|e| !e.dropped).map(|_| u64::from(id))
    }
    /// Share only this owner's high-VA SPTs with the existing native channel.
    /// Tables stay in this owner; collection invalidates the two directories
    /// before release. Low channel/bootstrap mappings stay private.
    pub fn bind(&mut self, io: &mut impl Io, directory: u64, instance: u64, epoch: u64) -> Result<(), &'static str> {
        if self.alias.is_some_and(|old| old != directory) { return Err("native resource channel changed"); }
        let Some(space) = self.space.as_ref() else { return Err("native resource VM absent"); };
        if !space.usable() { return Err("native resource VM uncertain"); }
        self.alias = Some(directory);
        if self.incarnation != Some((instance,epoch)) {
            self.channel = self.channel.checked_add(1).filter(|id| *id < 1<<32)
                .ok_or("native public context namespace exhausted")?;
            self.incarnation = Some((instance,epoch));
        }
        for (index,_) in space.table_ranges() {
            let value = io.read(space.directory.range.start + index*8)?;
            let old = io.read(directory+index*8)?;
            if old != 0 && old != value { return Err("native resource PDE alias occupied"); }
            if !self.alias_indices.contains(&index) { self.alias_indices.push(index); }
            io.write(directory+index*8,value)?;
        }
        io.flush()?;
        io.invalidate(directory)?;
        Ok(())
    }
    #[cfg(target_os = "none")]
    pub(in crate::gk208) fn draw_buffer(&mut self, a: &mut Allocator, io: &mut impl Io,
        bytes: u32,
    ) -> Result<u32, &'static str> {
        let id = self.temporary;
        self.temporary = id.checked_sub(1).ok_or("native staging IDs exhausted")?;
        self.next_va = self.next_va.next_multiple_of(1<<17);
        self.create(a,io,id,3,0,bytes,1)?;
        Ok(id)
    }
    pub fn staging(&mut self, a: &mut Allocator, io: &mut impl Io, format: u32,
        width: u32, height: u32,
    ) -> Result<u32, &'static str> {
        let id = self.temporary;
        self.temporary = id.checked_sub(1).ok_or("native staging IDs exhausted")?;
        self.create(a,io,id,2,format,width,height)?;
        Ok(id)
    }
    pub fn unref(&mut self, id: u32) -> Result<(), &'static str> {
        self.entries.get_mut(&id).ok_or("native resource absent")?.dropped = true;
        Ok(())
    }
    pub fn collect(&mut self, a: &mut Allocator, io: &mut impl Io) -> Result<(), &'static str> {
        let before = a.stats().charged;
        let result = (|| {
            let ready: Vec<_> = self.entries.iter().filter(|(_,e)| e.dropped && Arc::strong_count(&e.reference) == 1)
                .map(|(id,_)| *id).collect();
            for id in ready {
                let e = &self.entries[&id];
                if e.mapped {
                    self.space.as_mut().ok_or("native resource mapping absent")?
                        .unmap(io, e.va, e.bytes.next_multiple_of(4096))?;
                } else if e.allocation.is_some() || e.backing.is_some() {
                    // A failed map may have published only part of the extent.
                    // Retain the allocation and pinned backing on this uncertain result.
                    return Err("native resource mapping uncertain; storage retained");
                }
                if let Some(va) = e.backing_va {
                    // Arc<B> is retained until both PDB invalidations complete.
                    let bytes = e.backing_bytes;
                    self.space.as_mut().unwrap().unmap(io,va,bytes)?;
                }
                if let Some(directory) = self.alias { io.invalidate(directory)?; }
                let mut e = self.entries.remove(&id).unwrap();
                e.pin.take();
                if let Some(allocation) = e.allocation { a.release(allocation)?; }
            }
            if self.entries.is_empty() {
                if let Some(directory) = self.alias {
                    for &index in &self.alias_indices { io.write(directory+index*8,0)?; }
                    io.flush()?;
                    io.invalidate(directory)?;
                    self.alias = None;
                    self.alias_indices.clear();
                }
                if let Some(space) = self.space.as_mut() { space.detach(io)?; }
                if let Some(space) = self.space.take() { space.reclaim(a)?; }
            }
            Ok(())
        })();
        self.account(before, a.stats().charged);
        result
    }
}
impl<B> Drop for Resources<B> {
    fn drop(&mut self) {
        // Owner destruction is not a hardware drain. Preserve pinned system
        // backing and allocation pins if explicit collection did not retire it.
        if !self.entries.is_empty() || self.space.is_some() {
            core::mem::forget(core::mem::take(&mut self.entries));
        }
    }
}


#[cfg(target_os = "none")]
mod native {
    use alloc::sync::Arc;
    use crate::gpu::{dev::Tag, dma::Pages};
    use super::super::{adapter::Request, ce};

    pub fn resource_create(tag: Tag, id: u32, kind: u32, format: u32,
        width: u32, height: u32,
    ) -> Option<Request> {
        ce::with_resources(|r, a, io| r.create(a, io, id, kind, format, width, height)).ok()?;
        Some(Request::storage(tag, None))
    }
    pub fn attach_backing(tag: Tag, id: u32, backing: Arc<Pages>) -> Option<Request> {
        let pages: alloc::vec::Vec<_> = (0..backing.count()).map(|i| backing.page_addr(i)).collect();
        ce::with_resources(|r, a, io| r.attach(a, io, id, backing.clone(), &pages, backing.bytes() as u64)).ok()?;
        Some(Request::storage(tag, Some(backing)))
    }
    pub fn resource_unref(tag: Tag, id: u32, backing: Option<Arc<Pages>>) -> Option<Request> {
        ce::with_resources(|r, a, io| {
            // Creation can have failed before a native entry was allocated.
            if r.address(id).is_some() { r.unref(id)?; }
            r.collect(a, io)
        }).ok()?;
        Some(Request::storage(tag, backing))
    }
}
#[cfg(target_os = "none")]
pub use native::{resource_create, attach_backing, resource_unref};
