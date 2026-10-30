/*
 * Copyright 2012, 2021 Red Hat Inc.
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
/*******************************************************************************
    Copyright (c) 2020, NVIDIA CORPORATION. All rights reserved.

    Permission is hereby granted, free of charge, to any person obtaining a
    copy of this software and associated documentation files (the "Software"),
    to deal in the Software without restriction, including without limitation
    the rights to use, copy, modify, merge, publish, distribute, sublicense,
    and/or sell copies of the Software, and to permit persons to whom the
    Software is furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in
    all copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
    THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
    FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
    DEALINGS IN THE SOFTWARE.

*******************************************************************************/

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

//! Worker-side attribution of the control's real unmapped-address write.
//! MMU fields/ack: MIT nouveau v6.8 fifo/gf100.c:696-729; reasons:
//! fifo/gk104.c:504-522. Ordered writes: MIT NVIDIA cla197.h.
use alloc::vec::Vec;

// The final 128 MiB PDE is checked absent in this channel before publication.
// Faults in the retained G18 capture report page addresses, so use one page
// token per sequence modulo 32768. Four in-flight sequences cannot alias; the
// adapter matches the token to a retained request in the current incarnation.
const FAULT_BASE: u64 = (1 << 40) - (1 << 27);
pub fn address(sequence: u32) -> Result<u64, &'static str> {
    if sequence == 0 { return Err("MMU control sequence extent"); }
    Ok(FAULT_BASE + u64::from(sequence % 32768) * 4096)
}

/// The first ordered report writes to an unmapped VA. The following reports
/// are valid retained receipts; the MMU event alone cannot release backing.
pub fn commands(target: u64, epoch: u64, sequence: u32) -> Result<Vec<u32>, &'static str> {
    if target == 0 || target & 3 != 0 || target.checked_add(12).is_none_or(|end| end > 1 << 40)
        || epoch == 0 { return Err("invalid MMU-error receipt"); }
    let invalid = address(sequence)?;
    let mut words = alloc::vec![0x20010000, 0xa197, 0x20010044, 0,
        0x200406c0, (invalid >> 32) as u32, invalid as u32, sequence, 0x1000f010];
    for (offset, value) in [(4, epoch as u32), (8, (epoch >> 32) as u32), (0, sequence)] {
        let address = target + offset;
        words.extend([0x200406c0, (address >> 32) as u32, address as u32, value, 0x1000f010]);
    }
    Ok(words)
}

pub trait Io {
    fn read(&mut self, address: usize) -> u32;
    fn write(&mut self, address: usize, value: u32);
    fn refuse_trap(&mut self, words: [u32; 4]) -> Result<(), &'static str>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trap {
    pub instance: u64,
    pub page: u32,
    pub status: u32,
    pub raw: [u32; 4],
}
/// Read before W1C. Foreign engines, PGRAPH exceptions, unexpected addresses
/// and non-PDE/PTE faults retain the confirmed-reset path.
pub fn capture(io: &mut impl Io) -> Result<Option<Trap>, &'static str> {
    let words = [0x400100, 0x400108, 0x400110, 0x400118].map(|r| io.read(r));
    if words != [0; 4] {
        io.refuse_trap(words)?;
        return Err("unhandled PGRAPH fault; backing retained");
    }
    let mask = io.read(0x259c);
    if mask == 0 { return Ok(None); }
    // Only GR unit 0 can produce this A197 report write.
    if mask != 1 { return Err("foreign MMU fault; backing retained"); }
    let raw = core::array::from_fn(|n| io.read(0x2800 + n * 4));
    let va = u64::from(raw[1]) | u64::from(raw[2]) << 32;
    let page = va.checked_sub(FAULT_BASE).and_then(|v| u32::try_from(v / 4096).ok())
        .ok_or("unexpected MMU address; backing retained")?;
    let reason = raw[3] & 15;
    if raw[0] == 0 || page >= 32768 || va != FAULT_BASE + u64::from(page) * 4096 || raw[3] & 0x80 == 0 || !matches!(reason, 0 | 2) {
        return Err("unexpected MMU write/reason; backing retained");
    }
    Ok(Some(Trap { instance: u64::from(raw[0]) << 12,
        page, status: 0x60010000 | reason, raw }))
}
/// G19: MIT nouveau v6.8 fifo/runl.c:58-141 and gk104.c:39-45,
/// 404-419. Block before acknowledging; never resume a faulted GR channel.
/// The caller must reset GR and commit the surviving runlist before clearing
/// the scheduler exception and unblocking. This alone releases no backing.
pub fn acknowledge(io: &mut impl Io, trap: Trap, channel: u32, runlist: u32) -> Result<(), &'static str> {
    if channel >= 1024 || runlist >= 32 { return Err("MMU recovery channel/runlist extent"); }
    if capture(io)? != Some(trap) { return Err("MMU fault changed; backing retained"); }
    let blocked = io.read(0x2630) | (1 << runlist);
    io.write(0x2630, blocked);
    io.write(0x259c, 1);
    let address = 0x800004 + channel as usize * 8;
    let disabled = io.read(address) | 0x800;
    io.write(address, disabled);
    if io.read(0x259c) != 0 { return Err("MMU acknowledgement failed; backing retained"); }
    Ok(())
}
