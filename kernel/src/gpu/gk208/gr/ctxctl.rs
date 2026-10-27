/*
 * Copyright 2010 Red Hat Inc.
 * Copyright 2011 Red Hat Inc.
 * Copyright 2012 Red Hat Inc.
 * Copyright 2013 Red Hat Inc.
 * Copyright 2015 Red Hat Inc.
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
//! GK208B source-built ctxctl protocol. Runs in the suite thread, outside IRQs.
//! Adapted from nouveau v6.8 gf100.c (init, init_ctxctl_int, init_csdata),
//! gk208.c/gk104.c/gf117.c initialization and hub.fuc/gpc.fuc (MIT).
//! Power-gating exit: nvkm/subdev/pmu/gk110.c:32-83, selected by
//! pmu/gk208.c and called before topology by gr/gf100.c:2006 (MIT).
//! Register ports: envytools f102b823 falcon.xml/ctxctl.xml (MIT).
//! The external NVIDIA firmware protocol is incompatible with these images.

use super::{firmware, tables, topology::Topology};
use crate::block::install::sha256::Sha256;
use alloc::vec::Vec;

pub const BOUND_US: u64 = 2_000_000;
const MAX_POLLS: usize = 2_000_000;
const FECS: u32 = 0x409000;
const GPCCS: u32 = 0x502000;
const GPC_BCAST: u32 = 0x41a000;
const READY: u32 = 0x80000000;
const START_POISON: u32 = 0x15555555;

pub trait Access {
    fn read(&mut self, address: u32) -> u32;
    fn write(&mut self, address: u32, value: u32);
    fn now_us(&mut self) -> u64;
    fn relax(&mut self);
}

#[derive(Debug)]
pub struct Fault {
    pub reason: &'static str,
    pub elapsed_us: u64,
    pub address: u32,
    pub value: u32,
}
fn fault(reason: &'static str, address: u32, value: u32) -> Fault {
    Fault {
        reason,
        elapsed_us: 0,
        address,
        value,
    }
}
fn mask(io: &mut impl Access, address: u32, bits: u32, value: u32) {
    let old = io.read(address);
    io.write(address, (old & !bits) | value);
}

// Bound by elapsed time; only consecutive clock stalls consume the guard.
fn poll(
    io: &mut impl Access,
    bound: u64,
    reason: &'static str,
    mut sample: impl FnMut(&mut dyn Access) -> (bool, u32, u32),
) -> Result<u64, Fault> {
    let begin = io.now_us();
    let mut last_time = begin;
    let mut stalled = 0;
    loop {
        let (done, address, value) = sample(io);
        let now = io.now_us();
        let elapsed = now.saturating_sub(begin);
        if now > last_time {
            last_time = now;
            stalled = 0;
        } else {
            stalled += 1;
        }
        if done && elapsed > 0 && elapsed <= bound {
            return Ok(elapsed);
        }
        if elapsed >= bound || stalled >= MAX_POLLS {
            return Err(Fault {
                reason,
                elapsed_us: elapsed,
                address,
                value,
            });
        }
        io.relax();
    }
}
fn delay(io: &mut impl Access, us: u64) -> Result<(), Fault> {
    let begin = io.now_us();
    poll(io, BOUND_US, "GR reset delay clock stalled", |io| {
        (
            io.now_us().saturating_sub(begin) >= us,
            0x200,
            io.read(0x200),
        )
    })?;
    Ok(())
}

/// Reset PGRAPH and leave its power-gated units accessible before discovery.
/// PFIFO/CE and display enables are outside the masks used here.
pub fn reset(io: &mut impl Access) -> Result<(), Fault> {
    let temporary = io.read(0x200) & 0x08000000;
    mask(io, 0x200, 0x1000, 0);
    if io.read(0x200) & 0x1000 != 0 {
        return Err(fault("GR reset did not assert", 0x200, io.read(0x200)));
    }
    mask(io, 0x200, 0x08000000, 0x08000000);
    let result = (|| {
        delay(io, 50_000)?;
        mask(io, 0x10a78c, 2, 2);
        mask(io, 0x10a78c, 1, 1);
        mask(io, 0x10a78c, 1, 0);
        mask(io, 0x206b4, 0, 0);
        // gk110_pmu_pgob(false), also selected by GK208B. A failed PRI
        // response is not truncated into a topology mask or a completion.
        for (address, value) in [
            (0x20520, 0xfffffffc),
            (0x20524, 0xfffffffe),
            (0x20524, 0xfffffffc),
            (0x20524, 0xfffffff8),
            (0x20524, 0xffffffe0),
            (0x20530, 0xfffffffe),
            (0x2052c, 0xfffffffa),
            (0x2052c, 0xfffffff0),
            (0x2052c, 0xffffffc0),
            (0x2052c, 0xffffff00),
            (0x2052c, 0xfffffc00),
            (0x2052c, 0xfffcfc00),
            (0x2052c, 0xfff0fc00),
            (0x2052c, 0xff80fc00),
            (0x20528, 0xfffffffe),
            (0x20528, 0xfffffffc),
        ] {
            io.write(address, value);
            poll(io, BOUND_US, "GR power-gating exit timeout", |io| {
                let value = io.read(address);
                (value & 0x80000000 == 0, address, value)
            })?;
        }
        Ok(())
    })();
    // Do not leave the temporary power-gating controls asserted on refusal.
    mask(io, 0x10a78c, 2, 0);
    mask(io, 0x10a78c, 1, 1);
    mask(io, 0x10a78c, 1, 0);
    mask(io, 0x200, 0x08000000, temporary);
    result?;
    mask(io, 0x200, 0x1000, 0x1000);
    delay(io, 10)?;
    if io.read(0x200) & 0x1000 == 0 {
        return Err(fault("GR enable did not latch", 0x200, io.read(0x200)));
    }
    Ok(())
}

fn init(io: &mut impl Access, t: &Topology<'_>, mmu: [u64; 2]) -> Result<(), Fault> {
    // This first startup is board-specific. Do not silently invent tile maps
    // for another chip; GK208B has one GPC and at most two active TPCs.
    if t.gpc_nr != 1 || !(1..=2).contains(&t.tpc_nr[0]) {
        return Err(fault(
            "unsupported GK208B startup topology",
            0x409604,
            io.read(0x409604),
        ));
    }
    if mmu
        .iter()
        .any(|a| *a == 0 || *a >= 1 << 40 || *a & 0xfff != 0)
        || mmu[0] == mmu[1]
    {
        return Err(fault("invalid GR MMU scratch backing", 0x4188b4, 0));
    }
    mask(io, 0x400500, 0x00010001, 0);
    let fb = io.read(0x100c80) & 1;
    io.write(0x418880, fb);
    io.write(0x4188a4, 0x03000000);
    for address in [0x418888, 0x41888c, 0x418890, 0x418894] {
        io.write(address, 0);
    }
    io.write(0x4188b4, (mmu[0] >> 8) as u32);
    io.write(0x4188b8, (mmu[1] >> 8) as u32);
    for pack in tables::INIT_LISTS[0].packs {
        for entry in pack.init {
            for i in 0..entry.count {
                io.write(entry.addr + i * entry.pitch, entry.data);
            }
        }
    }
    poll(io, BOUND_US, "GR idle timeout", |io| {
        io.read(0x400700); // updates FIFO_ENGINE_STATUS
        let busy = io.read(0x40060c);
        (busy & 1 == 0, 0x40060c, busy)
    })?;
    io.write(0x503018, 1); // gk104 VSC stream master
    let count = u32::from(t.tpc_nr[0]);
    let magic = 0x00800000u32.div_ceil(count);
    // gf117 zcull, one GPC: tile row offset 1; banks 0 then 1.
    for i in 0..4 {
        io.write(
            0x418980 + i * 4,
            if i == 0 && count == 2 { 0x10 } else { 0 },
        );
    }
    io.write(0x500914, 0x100 | count);
    io.write(0x500910, 0x00040000 | count);
    io.write(0x500918, magic);
    io.write(0x41bfd4, magic);
    let ltcs = io.read(0x100800);
    io.write(0x4188ac, ltcs);
    let fbps = io.read(0x120074);
    mask(io, 0x408850, 0xf, fbps & 0xf);
    mask(io, 0x408958, 0xf, fbps & 0xf);
    io.write(0x400500, 0x00010001);
    for a in [0x400100, 0x40013c] {
        io.write(a, u32::MAX);
    }
    io.write(0x400124, 2);
    io.write(0x409c24, 0x000e0001); // internal, unmodified source firmware
    for a in [0x404000, 0x404600, 0x408030, 0x406018, 0x404490, 0x405840] {
        io.write(a, 0xc0000000);
    }
    io.write(0x407020, 0x40000000);
    io.write(0x405844, 0x00ffffff);
    mask(io, 0x419cc0, 8, 8);
    io.write(0x503038, 0xc0000000); // PPC0
    for a in [0x500420, 0x500900, 0x501028, 0x500824] {
        io.write(a, 0xc0000000);
    }
    for tpc in 0..8 {
        if t.tpc_mask[0] & (1 << tpc) == 0 {
            continue;
        }
        let base = 0x504000 + tpc * 0x800;
        for offset in [0x508, 0x50c] {
            io.write(base + offset, u32::MAX);
        }
        for offset in [0x224, 0x084, 0x48c] {
            io.write(base + offset, 0xc0000000);
        }
        io.write(base + 0x644, 0x001ffffe);
        io.write(base + 0x64c, 0x0000000f);
    }
    for a in [0x502c90, 0x502c94] {
        io.write(a, u32::MAX);
    }
    let rops = (io.read(0x409604) >> 16) & 0x1f;
    if rops == 0 || rops > 8 {
        return Err(fault("GR ROP count out of range", 0x409604, rops));
    }
    for rop in 0..rops {
        let base = 0x410000 + rop * 0x400;
        for offset in [0x144, 0x070] {
            io.write(base + offset, 0x40000000);
        }
        for offset in [0x204, 0x208] {
            io.write(base + offset, u32::MAX);
        }
    }
    for a in [0x400108, 0x400138, 0x400118, 0x400130, 0x40011c, 0x400134] {
        io.write(a, u32::MAX);
    }
    io.write(0x400054, 0x34ce3464);
    Ok(())
}

fn capacity(io: &mut impl Access, base: u32, code: usize, data: usize) -> Result<u32, Fault> {
    let caps = io.read(base + 0x108);
    let imem = (caps & 0x1ff) * 256;
    let dmem = ((caps >> 9) & 0x1ff) * 256;
    if code * 4 > imem as usize || data * 4 > dmem as usize || dmem > 0x10000 {
        return Err(fault(
            "Falcon image exceeds hardware capacity",
            base + 0x108,
            caps,
        ));
    }
    Ok(dmem)
}
fn load(io: &mut impl Access, base: u32, words: &[u32], code: bool) {
    let index = if code { 0x180 } else { 0x1c0 };
    io.write(base + index, 0x01000000);
    for (i, word) in words.iter().enumerate() {
        if code && i % 64 == 0 {
            io.write(base + 0x188, (i / 64) as u32);
        }
        io.write(base + index + 4, *word);
    }
}
fn verify(io: &mut impl Access, base: u32, words: &[u32], code: bool) -> Result<[u8; 32], Fault> {
    let index = if code { 0x180 } else { 0x1c0 };
    io.write(base + index, 0x02000000);
    let mut hash = Sha256::new();
    for word in words {
        let actual = io.read(base + index + 4);
        if actual != *word {
            return Err(fault(
                "Falcon upload read-back mismatch",
                base + index + 4,
                actual,
            ));
        }
        hash.update(&actual.to_le_bytes());
    }
    Ok(hash.finalize())
}

fn read_data(io: &mut impl Access, base: u32, offset: u32) -> u32 {
    io.write(base + 0x1c0, offset);
    io.read(base + 0x1c4)
}
fn write_data(io: &mut impl Access, base: u32, offset: u32, data: u32) -> Result<(), Fault> {
    io.write(base + 0x1c0, offset);
    io.write(base + 0x1c4, data);
    let read_base = if base == GPC_BCAST { GPCCS } else { base };
    if read_data(io, read_base, offset) != data {
        return Err(fault(
            "ctxctl register list read-back mismatch",
            read_base + 0x1c4,
            offset,
        ));
    }
    Ok(())
}
// Encode address runs, not init values: gf100_gr_init_csdata.
fn csdata(
    io: &mut impl Access,
    packs: &[tables::Pack],
    base: u32,
    pointer: u32,
    origin: u32,
    dmem: u32,
) -> Result<(), Fault> {
    let read_base = if base == GPC_BCAST { GPCCS } else { base };
    let head = read_data(io, read_base, pointer);
    let tail = read_data(io, read_base, pointer + 4);
    let mut star = head.max(tail);
    if star & 3 != 0 || star >= dmem {
        return Err(fault("invalid ctxctl list pointer", base + 0x1c4, star));
    }
    let mut encoded = Vec::new();
    let mut address = 0;
    let mut previous = u32::MAX;
    let mut count = 0u32;
    for pack in packs {
        for entry in pack.init {
            for i in 0..entry.count {
                let next = entry
                    .addr
                    .checked_sub(origin)
                    .and_then(|v| v.checked_add(i * entry.pitch))
                    .ok_or_else(|| fault("ctxctl list address overflow", entry.addr, origin))?;
                if next != previous.wrapping_add(4) || count >= 32 {
                    if count != 0 {
                        encoded.push(((count - 1) << 26) | address);
                    }
                    address = next;
                    count = 0;
                }
                previous = next;
                count += 1;
            }
        }
    }
    if count != 0 {
        encoded.push(((count - 1) << 26) | address);
    }
    if star as usize + encoded.len() * 4 > dmem as usize {
        return Err(fault(
            "ctxctl register list exceeds DMEM",
            base + 0x1c4,
            star,
        ));
    }
    for word in encoded {
        write_data(io, base, star, word)?;
        star += 4;
    }
    write_data(io, base, pointer + 4, star)
}

#[derive(Debug)]
pub struct Ready {
    pub status: Vec<[u32; 2]>,
    pub elapsed_us: u64,
    pub size: u32,
}
#[derive(Debug)]
pub struct Reply {
    pub method: u32,
    pub poison: u32,
    pub reply: u32,
    pub size: u32,
    pub elapsed_us: u64,
}
#[derive(Debug)]
pub struct Control {
    pub method: u32,
    pub started: bool,
    pub ready: bool,
    pub accepted: bool,
    pub poison: u32,
    pub planted: u32,
    pub reply: u32,
    pub elapsed_us: u64,
}
pub struct Controller {
    pub readback_sha256: [[u8; 32]; 4],
    started: bool,
    size: u32,
}
impl Controller {
    pub fn upload(io: &mut impl Access, t: &Topology<'_>, mmu: [u64; 2]) -> Result<Self, Fault> {
        init(io, t, mmu)?;
        let hub_capacity = capacity(
            io,
            FECS,
            firmware::FECS_CODE_BIN.len(),
            firmware::FECS_DATA_BIN.len(),
        )?;
        let gpc_capacity = capacity(
            io,
            GPCCS,
            firmware::GPCCS_CODE_BIN.len(),
            firmware::GPCCS_DATA_BIN.len(),
        )?;
        load(io, FECS, &firmware::FECS_DATA_BIN, false);
        load(io, FECS, &firmware::FECS_CODE_BIN, true);
        load(io, GPC_BCAST, &firmware::GPCCS_DATA_BIN, false);
        load(io, GPC_BCAST, &firmware::GPCCS_CODE_BIN, true);
        let readback_sha256 = [
            verify(io, FECS, &firmware::FECS_CODE_BIN, true)?,
            verify(io, FECS, &firmware::FECS_DATA_BIN, false)?,
            verify(io, GPCCS, &firmware::GPCCS_CODE_BIN, true)?,
            verify(io, GPCCS, &firmware::GPCCS_DATA_BIN, false)?,
        ];
        for (list, base, pointer, origin, cap) in [
            (3, FECS, 0, 0, hub_capacity),
            (4, GPC_BCAST, 0, 0x418000, gpc_capacity),
            (5, GPC_BCAST, 0, 0x418000, gpc_capacity),
            (6, GPC_BCAST, 4, 0x419800, gpc_capacity),
            (7, GPC_BCAST, 8, 0x41be00, gpc_capacity),
        ] {
            csdata(
                io,
                tables::INIT_LISTS[list].packs,
                base,
                pointer,
                origin,
                cap,
            )?;
        }
        io.write(FECS + 0x800, START_POISON);
        io.write(GPCCS + 0x800, 0);
        io.write(FECS + 0x804, 0);
        Ok(Self {
            readback_sha256,
            started: false,
            size: 0,
        })
    }
    fn wait_ready(&mut self, io: &mut impl Access) -> Result<Ready, Fault> {
        let elapsed_us = poll(io, BOUND_US, "ctxctl ready timeout", |io| {
            let fecs = io.read(FECS + 0x800);
            let gpc = io.read(GPCCS + 0x800);
            (
                self.started && fecs & READY != 0 && gpc & READY != 0,
                FECS + 0x800,
                fecs,
            )
        })?;
        let size = io.read(FECS + 0x804);
        if size == 0 || size > 1 << 20 || size & 3 != 0 {
            return Err(fault(
                "invalid ctxctl context image size",
                FECS + 0x804,
                size,
            ));
        }
        self.size = size;
        Ok(Ready {
            status: alloc::vec![
                [FECS + 0x800, io.read(FECS + 0x800)],
                [GPCCS + 0x800, io.read(GPCCS + 0x800)]
            ],
            elapsed_us,
            size,
        })
    }
    pub fn start(&mut self, io: &mut impl Access) -> Result<Ready, Fault> {
        if self.started {
            return Err(fault(
                "ctxctl already started",
                FECS + 0x100,
                io.read(FECS + 0x100),
            ));
        }
        io.write(FECS + 0x104, 0);
        io.write(FECS + 0x10c, 0);
        io.write(FECS + 0x800, 0);
        self.started = true;
        io.write(FECS + 0x100, 2); // hub.fuc starts GPCCS; never race its context offset
        self.wait_ready(io)
    }
    pub fn withheld_start(&mut self, io: &mut impl Access) -> Result<Control, Fault> {
        if self.started {
            return Err(fault("withheld start after start", FECS + 0x100, 2));
        }
        let error = match self.wait_ready(io) {
            Err(error) => error,
            Ok(_) => {
                return Err(fault(
                    "unstarted ctxctl reported ready",
                    FECS + 0x800,
                    io.read(FECS + 0x800),
                ))
            }
        };
        let reply = io.read(FECS + 0x800);
        if reply != START_POISON || io.read(GPCCS + 0x800) != 0 || error.elapsed_us < BOUND_US {
            return Err(Fault {
                reason: "withheld ctxctl start control failed",
                elapsed_us: error.elapsed_us,
                address: FECS + 0x800,
                value: reply,
            });
        }
        Ok(Control {
            method: 0,
            started: false,
            ready: false,
            accepted: false,
            poison: START_POISON,
            planted: 0,
            reply,
            elapsed_us: error.elapsed_us,
        })
    }
    fn wait_reply(
        &mut self,
        io: &mut impl Access,
        method: u32,
        poison: u32,
    ) -> Result<Reply, Fault> {
        let elapsed_us = poll(io, BOUND_US, "ctxctl mailbox timeout", |io| {
            let reply = io.read(FECS + 0x800);
            (
                reply != poison && reply == poison | READY,
                FECS + 0x800,
                reply,
            )
        })?;
        let size = io.read(FECS + 0x804);
        if size == 0 || size != self.size {
            return Err(fault("ctxctl mailbox size changed", FECS + 0x804, size));
        }
        Ok(Reply {
            method,
            poison,
            reply: io.read(FECS + 0x800),
            size,
            elapsed_us,
        })
    }
    fn clear_reply(io: &mut impl Access, poison: u32) -> Result<(), Fault> {
        io.write(FECS + 0x840, READY);
        let reply = io.read(FECS + 0x800);
        if reply != poison {
            return Err(fault("ctxctl reply clear failed", FECS + 0x800, reply));
        }
        Ok(())
    }
    /// Caller supplies a valid instance, with context backing already pinned.
    pub fn request(
        &mut self,
        io: &mut impl Access,
        method: u32,
        instance: u32,
        poison: u32,
    ) -> Result<Reply, Fault> {
        if !self.started || self.size == 0 || ![1, 2].contains(&method) || poison & READY != 0 {
            return Err(fault("invalid ctxctl request", FECS + 0x504, method));
        }
        io.write(FECS + 0x800, poison);
        if io.read(FECS + 0x800) != poison {
            return Err(fault(
                "ctxctl poison did not latch",
                FECS + 0x800,
                io.read(FECS + 0x800),
            ));
        }
        Self::clear_reply(io, poison)?;
        io.write(FECS + 0x500, instance);
        io.write(FECS + 0x504, method);
        self.wait_reply(io, method, poison)
    }
    pub fn stale_reply(&mut self, io: &mut impl Access, previous: u32) -> Result<Control, Fault> {
        if !self.started || self.size == 0 || previous & READY == 0 {
            return Err(fault("invalid stale reply control", FECS + 0x800, previous));
        }
        let poison = previous & !READY;
        io.write(FECS + 0x800, poison);
        io.write(FECS + 0x800, previous);
        Self::clear_reply(io, poison)?;
        let error = match self.wait_reply(io, 1, poison) {
            Err(error) => error,
            Ok(_) => {
                return Err(fault(
                    "withheld ctxctl request accepted",
                    FECS + 0x800,
                    io.read(FECS + 0x800),
                ))
            }
        };
        let reply = io.read(FECS + 0x800);
        if reply != poison || error.elapsed_us < BOUND_US {
            return Err(Fault {
                reason: "stale ctxctl reply control failed",
                elapsed_us: error.elapsed_us,
                address: FECS + 0x800,
                value: reply,
            });
        }
        Ok(Control {
            method: 1,
            started: true,
            ready: true,
            accepted: false,
            poison,
            planted: previous,
            reply,
            elapsed_us: error.elapsed_us,
        })
    }
}
