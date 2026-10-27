/*
 * Copyright 2010 Red Hat Inc.
 * Copyright 2011 Red Hat Inc.
 * Copyright 2012 Red Hat Inc.
 * Copyright 2013 Red Hat Inc.
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
/*
 * rnndb copyright 2006
 * Artur Huillet
 * Ben Skeggs
 * B. R.
 * Carlos Martin
 * Christoph Bumiller
 * Dawid Gajownik
 * Dmitry Baryshkov
 * Dmitry Eremin-Solenikov
 * EdB
 * Erik Waling
 * Francisco Jerez
 * Ilia Mirkin
 * jb17bsome
 * Jeremy Kolb
 * Laurent Carlier
 * Luca Barbieri
 * Maarten Maathuis
 * Marcelina Kościelnicka
 * Mark Carey
 * Matthieu Castet
 * nvidiaman
 * Patrice Mandin
 * Pekka Paalanen
 * Peter Popov
 * Richard Hughes
 * Rudi Cilibrasi
 * Serge Martin
 * Simon Raffeiner
 * Stephane Loeuillet
 * Stephane Marchesin
 * sturmflut
 * Sylvain Munaut
 * Victor Stinner
 * Wladmir van der Laan
 * Younes Manton
 *
 * Permission is hereby granted, free of charge, to any person obtaining
 * a copy of this software and associated documentation files (the
 * "Software"), to deal in the Software without restriction, including
 * without limitation the rights to use, copy, modify, merge, publish,
 * distribute, sublicense, and/or sell copies of the Software, and to
 * permit persons to whom the Software is furnished to do so, subject to
 * the following conditions:
 *
 * The above copyright notice and this permission notice (including the
 * next paragraph) shall be included in all copies or substantial
 * portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
 * EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
 * MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
 * IN NO EVENT SHALL THE COPYRIGHT OWNER(S) AND/OR ITS SUPPLIERS BE
 * LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
 * OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
 * WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */
//! GK208B golden context, adapted from the pinned MIT nouveau generate path.
//! Hardware storage and CE readback are supplied by the boot-thread owner.
//! Source dispatch: ctxgk208.c; mechanism: ctxgf100.c/ctxgf117.c/ctxgk104.c,
//! gf100.c, hub.fuc/com.fuc/macros.fuc; indexed ports: MIT rnndb falcon.xml.
use super::{ctxctl, tables, topology::Topology};
use crate::block::install::sha256::Sha256;
use alloc::{vec, vec::Vec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    pub name: &'static str,
    pub bytes: u64,
    pub align: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct Buffer {
    pub spec: Spec,
    pub va: u64,
}
#[derive(Debug)]
pub struct Error {
    pub reason: &'static str,
    pub offset: usize,
    pub first: u8,
    pub second: u8,
}
impl From<&'static str> for Error {
    fn from(reason: &'static str) -> Self {
        Self {
            reason,
            offset: 0,
            first: 0,
            second: 0,
        }
    }
}
impl From<ctxctl::Fault> for Error {
    fn from(f: ctxctl::Fault) -> Self {
        f.reason.into()
    }
}
fn difference(reason: &'static str, offset: usize, first: u8, second: u8) -> Error {
    Error {
        reason,
        offset,
        first,
        second,
    }
}
/// Backing stays pinned on errors until a confirmed GR reset and CE drain.
pub trait Io: ctxctl::Access {
    fn allocate(&mut self, spec: Spec) -> Result<Buffer, Error>;
    fn patch(&mut self, buffer: Buffer, pairs: &[[u32; 2]]) -> Result<(), Error>;
    fn point(&mut self, image: Buffer) -> Result<(), Error>;
    fn poison(&mut self, image: Buffer, byte: u8) -> Result<(), Error>;
    fn request(&mut self, method: u32, poison: u32) -> Result<ctxctl::Reply, Error>;
    /// Each image is CE-copied into pinned system pages under P and ~P.
    fn readback(&mut self, image: Buffer) -> Result<[Vec<u8>; 2], Error>;
    fn runnable(&mut self, image: Buffer, data: &[u8], header: [u32; 2])
        -> Result<[u32; 2], Error>;
}
#[derive(Debug)]
pub struct PackReceipt {
    pub count: usize,
    pub sha256: [u8; 32],
}
#[derive(Debug)]
pub struct Receipt {
    pub index: usize,
    pub count: usize,
    pub sha256: [u8; 32],
    pub packs: Vec<PackReceipt>,
}
#[derive(Clone, Copy, Debug)]
pub struct SaveLayout {
    pub gpc: u32,
    pub strands: u32,
    pub strand_base: u32,
    pub strand_words: u32,
}
impl SaveLayout {
    pub fn end(self, size: u32) -> Result<usize, Error> {
        // com.fuc:314-333: bases in 256-byte units, sizes in words;
        // one extra 256-byte block is reserved even for an exact multiple.
        let base = u64::from(self.strand_base) * 256;
        let end = base + u64::from(self.strand_words) * 4;
        let padded = base + (u64::from(self.strand_words >> 6) + 1) * 256;
        if self.gpc != 0
            || self.strands == 0
            || self.strands > 64
            || base < 256
            || end <= 256
            || end > u64::from(size)
            || padded != u64::from(size)
        {
            return Err("invalid saved-data size words".into());
        }
        Ok(end as usize)
    }
}
#[derive(Debug)]
pub struct Golden {
    pub buffers: Vec<Buffer>,
    pub patches: Vec<[u32; 2]>,
    pub lists: Vec<Receipt>,
    pub images: [Vec<u8>; 2],
    pub hashes: [[u8; 32]; 2],
    pub saved_end: usize,
    pub layout: SaveLayout,
    pub queries: Vec<ctxctl::Reply>,
    pub header: [u32; 2],
}
fn hash(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize()
}
fn word(hash: &mut Sha256, value: u32) {
    hash.update(&value.to_le_bytes());
}
fn mask(io: &mut impl ctxctl::Access, a: u32, bits: u32, v: u32) {
    let old = io.read(a);
    io.write(a, (old & !bits) | v);
}
fn wait(io: &mut impl ctxctl::Access, a: u32, bits: u32, wanted: u32) -> Result<(), Error> {
    let start = io.now_us();
    for _ in 0..2_000_000 {
        if io.read(a) & bits == wanted {
            return Ok(());
        }
        if io.now_us().saturating_sub(start) >= ctxctl::BOUND_US {
            break;
        }
        io.relax();
    }
    Err("GR context polled wait timed out".into())
}
fn delay(io: &mut impl ctxctl::Access) -> Result<(), Error> {
    let start = io.now_us();
    for _ in 0..2_000_000 {
        if io.now_us().saturating_sub(start) >= 10 {
            return Ok(());
        }
        io.relax();
    }
    Err("GR context delay clock stalled".into())
}
fn idle(io: &mut impl ctxctl::Access) -> Result<(), Error> {
    io.read(0x400700);
    wait(io, 0x40060c, 1, 0)
}
fn replay(io: &mut impl ctxctl::Access, index: usize) -> Result<Receipt, Error> {
    let list = &tables::INIT_LISTS[index];
    let mut h = Sha256::new();
    word(&mut h, list.packs.len() as u32);
    let mut count = 0;
    let mut packs = Vec::new();
    let mut previous = None;
    if index == 1 {
        io.write(0x400208, 0x80000000);
    }
    for pack in list.packs {
        let mut ph = Sha256::new();
        for w in [pack.class_id, pack.init.len() as u32] {
            word(&mut h, w);
            word(&mut ph, w);
        }
        for e in pack.init {
            count += 1;
            for w in [e.addr, e.count, e.pitch, e.data] {
                word(&mut h, w);
                word(&mut ph, w);
            }
            if (index == 1 || index == 2) && previous != Some(e.data) {
                io.write(if index == 1 { 0x400204 } else { 0x40448c }, e.data);
                previous = Some(e.data);
            }
            for i in 0..e.count {
                let a = e.addr + i * e.pitch;
                match index {
                    1 => {
                        io.write(0x400200, a);
                        if a & 0xffff == 0xe100 {
                            idle(io)?;
                        }
                        wait(io, 0x400700, 4, 0)?;
                    }
                    2 => io.write(0x404488, 0x80000000 | pack.class_id | (a << 14)),
                    _ => io.write(a, e.data),
                }
            }
        }
        packs.push(PackReceipt {
            count: pack.init.len(),
            sha256: ph.finalize(),
        });
    }
    if index == 1 {
        io.write(0x400208, 0);
    }
    Ok(Receipt {
        index,
        count,
        sha256: h.finalize(),
        packs,
    })
}
fn patches(io: &mut impl ctxctl::Access, b: &[Buffer], tpcs: u32) -> Vec<[u32; 2]> {
    let bundle = (b[0].va >> 8) as u32;
    let pool = (b[1].va >> 8) as u32;
    let attrib = (b[2].va >> 12) as u32;
    vec![
        [0x40800c, pool],
        [0x408010, 0x80000000],
        [0x419004, pool],
        [0x419008, 0],
        [0x4064cc, 0x80000000],
        [0x408004, bundle],
        [0x408008, 0x80000030],
        [0x418808, bundle],
        [0x41880c, 0x80000030],
        [0x4064c8, 0x00c20200],
        [0x418810, 0x80000000 | attrib],
        [0x419848, 0x10000000 | attrib],
        [0x405830, 0x02180648],
        [0x4064c4, 0x0192ffff],
        [0x5030c0, 0x10000000 | ((0x218 * tpcs) << 16)],
        [0x5030e4, ((0x648 * tpcs) << 16) | (0x324 * tpcs)],
        [0x17e91c, io.read(0x17e91c)],
        [0x17e920, io.read(0x17e920)],
    ]
}
fn floorsweep(io: &mut impl ctxctl::Access, t: &Topology<'_>) {
    let count = u32::from(t.tpc_nr[0]);
    let mut sm = 0;
    for tpc in 0..8 {
        if t.tpc_mask[0] & (1 << tpc) == 0 {
            continue;
        }
        let base = 0x504000 + tpc * 0x800;
        for a in [base + 0x698, base + 0x4e8, 0x500c10 + tpc * 4, base + 0x88] {
            io.write(a, sm);
        }
        sm += 1;
    }
    io.write(0x500c08, count);
    io.write(0x500c8c, count);
    for i in 0..4 {
        io.write(0x405870 + i * 4, if i == 0 { count } else { 0 });
        io.write(0x406028 + i * 4, if i == 0 { count } else { 0 });
    }
    // gf100 oneinit_tiles: one GPC means 32 tile IDs of value 0; row offset 1
    // for one/two TPCs. gf117 uses PPC broadcast addresses for the TP map.
    let shift = if count == 1 { 4 } else { 3 };
    io.write(0x418bb8, (count << 8) | 1);
    io.write(0x41bfd0, (count << 8) | 1 | (16 << 16) | (shift << 21));
    io.write(0x41bfe4, 0);
    io.write(0x4078bc, (count << 8) | 1);
    for i in 0..6 {
        for a in [0x418b08, 0x41bf00, 0x40780c] {
            io.write(a + i * 4, 0);
        }
    }
    for i in 0..32 {
        // gk104 alpha/beta assignment with GK208's sole PPC.
        let atarget = (count * i / 32).max(1);
        let btarget = count - atarget;
        let alpha = if atarget < btarget {
            atarget != 0
        } else {
            btarget == 0
        };
        io.write(
            0x406800 + i * 32,
            if alpha { u32::from(t.tpc_mask[0]) } else { 0 },
        );
        io.write(
            0x406c00 + i * 32,
            if alpha { 0 } else { u32::from(t.tpc_mask[0]) },
        );
    }
    for i in 0..8 {
        io.write(0x4064d0 + i * 4, 0);
    }
    io.write(0x405b00, (count << 8) | 1);
    mask(io, 0x419f78, 8, 0);
}
fn check_size(reply: &ctxctl::Reply, size: u32) -> Result<(), Error> {
    if reply.size != size {
        Err("FECS size changed".into())
    } else {
        Ok(())
    }
}
/// No captured context bytes or external firmware protocol enter this path.
pub fn generate(io: &mut impl Io, t: &Topology<'_>, size: u32) -> Result<Golden, Error> {
    if t.gpc_nr != 1 || !(1..=2).contains(&t.tpc_nr[0]) {
        return Err("unsupported GK208B context topology".into());
    }
    if size <= 256 || size > 1 << 20 || size & 3 != 0 || io.read(0x409804) != size {
        return Err("invalid FECS image size".into());
    }
    let mut buffers = Vec::new();
    for (name, bytes, align) in [
        ("bundle", 0x3000, 0x100),
        ("pagepool", 0x8000, 0x100),
        (
            "attrib",
            0x20 * (0x324 + 0x7ff) * u64::from(t.tpc_nr[0]),
            0x1000,
        ),
        ("mmio", 0x1000, 0x100),
        ("golden-55", u64::from(size), 0x100),
        ("golden-aa", u64::from(size), 0x100),
        ("runnable", u64::from(size), 0x100),
    ] {
        let spec = Spec { name, bytes, align };
        let b = io.allocate(spec)?;
        if b.spec != spec {
            return Err("allocated image size differs from FECS".into());
        }
        if b.va == 0
            || b.va & (align - 1) != 0
            || b.va.checked_add(bytes).is_none_or(|e| e > 1 << 40)
        {
            return Err("invalid context buffer address".into());
        }
        buffers.push(b);
    }
    // Capture the final GPC strand's actual size words, before the red switch.
    // Falcon v5 HOST_IO_INDEX_V4 selects indexed host register copies.
    let strands = io.read(0x502880);
    if strands == 0 || strands > 64 {
        return Err("invalid strand count".into());
    }
    let old_index = io.read(0x5020ac);
    io.write(0x5020ac, strands - 1);
    let layout = SaveLayout {
        gpc: 0,
        strands,
        strand_base: io.read(0x502908),
        strand_words: io.read(0x502910),
    };
    io.write(0x5020ac, old_index);
    let saved_end = layout.end(size)?;
    // ctxgf100 generate and gf100 fecs.reset: force FE power, red switch,
    // return to AUTO, initialise SCC RAM. Firmware remains source-built.
    io.write(0x404170, 0x12);
    wait(io, 0x404170, 0x10, 0)?;
    io.write(0x409614, 0x70);
    delay(io)?;
    mask(io, 0x409614, 0x700, 0x700);
    delay(io)?;
    io.read(0x409614);
    io.write(0x404170, 0x10);
    wait(io, 0x404170, 0x10, 0)?;
    io.write(0x40802c, 1);
    // Bind zero-initialised construction backing before ICMD. Method 1 selects
    // the VM/header only; it does not restore saved graphics state (hub.fuc).
    io.point(buffers[6])?;
    check_size(&io.request(1, 0x11111111)?, size)?;
    let mut lists = Vec::new();
    for index in [3, 4, 8, 5, 6, 7] {
        lists.push(replay(io, index)?);
    }
    idle(io)?;
    let idle_timeout = io.read(0x404154);
    io.write(0x404154, 0);
    let patches = patches(io, &buffers, u32::from(t.tpc_nr[0]));
    for [a, v] in &patches {
        io.write(*a, *v);
    }
    for (a, bit) in [
        (0x418c6c, 1),
        (0x41980c, 0x10),
        (0x41be08, 4),
        (0x4064c0, 0x80000000),
        (0x405800, 0x08000000),
        (0x419c00, 8),
    ] {
        mask(io, a, bit, bit);
    }
    floorsweep(io, t);
    idle(io)?;
    lists.push(replay(io, 1)?);
    io.write(0x404154, idle_timeout);
    lists.push(replay(io, 2)?);
    idle(io)?;
    // gk104 r418800 is disabled in the permitted source, so it has no writes.
    io.patch(buffers[3], &patches)?;
    let mut images = [Vec::new(), Vec::new()];
    let mut queries = Vec::new();
    for (n, poison) in [0x55, 0xaa].into_iter().enumerate() {
        let image = buffers[4 + n];
        io.poison(image, poison)?;
        io.point(image)?;
        for method in [1, 2] {
            let reply = io.request(method, if n == 0 { 0x15555555 } else { 0x2aaaaaaa })?;
            check_size(&reply, size)?;
            queries.push(reply);
        }
        let [first, second] = io.readback(image)?;
        if first.len() != size as usize || second.len() != size as usize {
            return Err("CE image size differs".into());
        }
        if let Some(i) = first.iter().zip(&second).position(|(a, b)| a != b) {
            return Err(difference("CE readbacks differ", i, first[i], second[i]));
        }
        for i in (0..256).chain(saved_end..size as usize) {
            if first[i] != poison {
                return Err(difference(
                    "save changed poison outside data",
                    i,
                    poison,
                    first[i],
                ));
            }
        }
        images[n] = first;
    }
    if let Some(i) = (256..saved_end).find(|i| images[0][*i] != images[1][*i]) {
        return Err(difference(
            "golden saves differ",
            i,
            images[0][i],
            images[1][i],
        ));
    }
    let hashes = [
        hash(&images[0][256..saved_end]),
        hash(&images[1][256..saved_end]),
    ];
    let header = [patches.len() as u32, (buffers[3].va >> 8) as u32];
    if io.runnable(buffers[6], &images[0], header)? != header {
        return Err("runnable header differs".into());
    }
    Ok(Golden {
        buffers,
        patches,
        lists,
        images,
        hashes,
        saved_end,
        layout,
        queries,
        header,
    })
}
