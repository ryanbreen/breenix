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

//! Worker-side handling of an attributed PGRAPH illegal method.
//! Hardware facts: nouveau v6.8 gf100.c:1613-1652,1693 (MIT), NVIDIA
//! cla197.h (MIT), and envytools rnndb gf100_pgraph/pgraph.xml (MIT).
use alloc::vec::Vec;

pub const ILLEGAL_METHOD: u32 = 0x10;
// A197 leaves 0x0148 undefined, between PM_TRIGGER_WFI (0x0144) and
// SET_INSTRUMENTATION_METHOD_HEADER (0x0150). It is not an MME method.
const UNDEFINED_METHOD: u32 = 0x148;

/// A197 waits for preceding work, then faults with the request sequence as
/// data. The ordered releases can execute only after the worker resumes FIFO.
pub fn commands(target: u64, epoch: u64, sequence: u32) -> Result<Vec<u32>, &'static str> {
    if target == 0 || target & 3 != 0 || target.checked_add(12).is_none_or(|end| end > 1 << 40)
        || epoch == 0 || sequence == 0 { return Err("invalid illegal-method receipt"); }
    let mut words = alloc::vec![0x20010000, 0xa197, 0x20010044, 0,
        0x20010000 | (UNDEFINED_METHOD >> 2), sequence];
    for (offset, value) in [(4, epoch as u32), (8, (epoch >> 32) as u32), (0, sequence)] {
        let address = target + offset;
        words.extend([0x200406c0, (address >> 32) as u32, address as u32, value, 0x1000f010]);
    }
    Ok(words)
}

pub trait Io {
    fn read(&mut self, address: usize) -> u32;
    fn write(&mut self, address: usize, value: u32);
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trap {
    pub instance: u64,
    pub sequence: u32,
    pub status: u32,
}
/// Read before W1C. Unknown or combined faults go to confirmed reset instead
/// of acknowledging another engine's state or manufacturing a completion.
pub fn capture(io: &mut impl Io) -> Result<Option<Trap>, &'static str> {
    let status = io.read(0x400100);
    let summary = io.read(0x400108);
    let gpcs = io.read(0x400118);
    if status == 0 && summary == 0 && gpcs == 0 { return Ok(None); }
    if status != ILLEGAL_METHOD || summary != 0 || gpcs != 0 {
        return Err("unhandled PGRAPH fault; backing retained");
    }
    let address = io.read(0x400704);
    if address & 0x3ffc != UNDEFINED_METHOD || (address >> 16) & 7 != 0
        || io.read(0x404200) != 0xa197 {
        return Err("unexpected PGRAPH method/class; backing retained");
    }
    Ok(Some(Trap { instance: u64::from(io.read(0x409b00) & 0x0fff_ffff) << 12,
        sequence: io.read(0x400708), status }))
}
/// Called only after the adapter finds this sequence on this channel/epoch.
/// The trap itself is not a release: request backing remains held until the
/// following semaphore and FIFO consumption, or the existing confirmed reset.
pub fn acknowledge(io: &mut impl Io, trap: Trap) -> Result<(), &'static str> {
    if capture(io)? != Some(trap) { return Err("PGRAPH trap changed; backing retained"); }
    io.write(0x400100, trap.status);
    io.write(0x400500, 0x00010001);
    if io.read(0x400100) != 0 || io.read(0x400108) != 0 || io.read(0x400118) != 0 {
        return Err("PGRAPH acknowledgement failed; backing retained");
    }
    Ok(())
}
