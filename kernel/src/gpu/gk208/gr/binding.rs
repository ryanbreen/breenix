/*
 * Copyright 2010 Red Hat Inc.
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
 * Copyright 2010 Christoph Bumiller
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

//! A197 program binding and suite-thread GR trap recovery.
// MIT cla197.h:2634-2638,3600-3619; Mesa nvc0_screen.c:895-898
// (upper/lower code address), nvc0_program.c:775-856 (SPH offsets).
use alloc::vec::Vec;
use super::{channel, shaders};

// MIT nouveau gf100.c:1414,1502,1614-1619: interrupt, trap and exception status.
pub const TRAP_REGISTERS: [usize; 4] = [0x400100, 0x400108, 0x400110, 0x400118];

/// Inspect status without altering it; capture and acknowledgement are separate.
pub fn check_traps(words: [u32; 4]) -> Result<(), &'static str> {
    if words != [0; 4] { return Err("shader binding PGRAPH trap/exception"); }
    Ok(())
}

/// Suite-thread access; this is not an interrupt handler.
pub trait TrapIo {
    fn read(&mut self, address: usize) -> u32;
    fn write(&mut self, address: usize, value: u32);
}

/// First failing method and binding, read before any W1C or FIFO resume.
pub struct TrapCapture {
    pub registers: [usize; 6],
    pub values: [u32; 6],
    pub status: [u32; 4],
}

pub fn capture_trap(io: &mut impl TrapIo) -> Option<TrapCapture> {
    let status = TRAP_REGISTERS.map(|a| io.read(a));
    // DATA_ERROR's code can remain latched after its interrupt is cleared.
    if status[0] == 0 && status[1] == 0 && status[3] == 0 { return None; }
    let address = io.read(0x400704);
    let registers = [0x400704, 0x400708, 0x40070c, 0x400500, 0x409b00,
        0x404200 + ((address >> 16) & 7) as usize * 4];
    let values = [address, io.read(registers[1]), io.read(registers[2]),
        io.read(registers[3]), io.read(registers[4]), io.read(registers[5])];
    Some(TrapCapture { registers, values, status })
}

/// MIT nouveau gf100.c:1235-1539,1572-1696, gf100.h:41-45.
/// Child status precedes its summary; DATA_ERROR precedes TRAP; resume last.
pub fn acknowledge_trap(io: &mut impl TrapIo, capture: &TrapCapture) {
    let mut status = capture.status[0];
    for bit in [1, 0x10, 0x20, 0x100000] {
        if status & bit != 0 { io.write(0x400100, bit); status &= !bit; }
    }
    let trap = capture.status[1];
    for (bit, address, value) in [
        (1, 0x404000, 0xc0000000), (2, 0x404600, 0xc0000000),
        (8, 0x408030, 0xc0000000), (0x10, 0x405840, 0xc0000000),
        (0x40, 0x40601c, 0xc0000000), (0x80, 0x404490, 0xc0000000),
        (0x100, 0x407020, 0x40000000),
    ] {
        if trap & bit != 0 {
            if bit != 0x100 || io.read(address) & 0x3fffffff != 0 { io.write(address, value); }
            io.write(0x400108, bit);
        }
    }
    if trap & 0x01000000 != 0 {
        let gpcs = capture.status[3];
        for gpc in 0..32 {
            if gpcs & (1 << gpc) == 0 { continue; }
            let base = 0x500000 + gpc * 0x8000;
            let units = io.read(base + 0x2c90);
            for (bit, offset) in [(1, 0x420), (2, 0x900), (4, 0x1028), (8, 0x824)] {
                if units & bit != 0 { io.write(base + offset, 0xc0000000); }
            }
            for tpc in 0..16 {
                let bit = 0x10000 << tpc;
                if units & bit == 0 { continue; }
                let t = base + 0x4000 + tpc * 0x800;
                let leaves = io.read(t + 0x508);
                for (leaf, offset) in [(1, 0x224), (4, 0x84), (8, 0x48c), (0x10, 0x430)] {
                    if leaves & leaf != 0 { io.write(t + offset, 0xc0000000); }
                }
                if leaves & 2 != 0 {
                    let global = io.read(t + 0x650);
                    io.write(t + 0x648, 0);
                    io.write(t + 0x650, global);
                }
                io.write(base + 0x2c90, bit);
            }
            io.write(0x400118, 1 << gpc);
        }
        io.write(0x400108, 0x01000000);
    }
    if trap & 0x02000000 != 0 {
        let rops = (io.read(0x409604) >> 16) & 0x1f;
        for rop in 0..rops as usize {
            for offset in [0x70, 0x144] { io.write(0x410000 + rop * 0x400 + offset, 0xc0000000); }
        }
        io.write(0x400108, 0x02000000);
    }
    let unknown = trap & !0x030001db;
    for bit in 0..32 {
        if unknown & (1 << bit) != 0 { io.write(0x400108, 1 << bit); }
    }
    if status & 0x200000 != 0 { io.write(0x400100, 0x200000); status &= !0x200000; }
    if status & 0x80000 != 0 {
        let fecs = io.read(0x409c18);
        for bit in 0..32 {
            if fecs & (1 << bit) != 0 { io.write(0x409c20, 1 << bit); }
        }
        io.write(0x400100, 0x80000); status &= !0x80000;
    }
    for bit in 0..32 {
        if status & (1 << bit) != 0 { io.write(0x400100, 1 << bit); }
    }
    io.write(0x400500, 0x00010001);
}

pub fn commands(
    code_address: u64, target: u64, epoch: u64, nonce: u32, reference: u32,
) -> Result<Vec<u32>, &'static str> {
    if code_address == 0 || code_address >= 1 << 40 || code_address & 255 != 0 {
        return Err("invalid shader code region");
    }
    let mut words = channel::bind().to_vec();
    // DMA_INCR on the same A197 subchannel as the report semaphore.
    let packet = |method: u32, count: u32| {
        0x20000000 | (count << 16) | (channel::SUBCHANNEL << 13) | (method / 4)
    };
    words.extend([packet(0x1608, 2), (code_address >> 32) as u32, code_address as u32]);
    for (slot, offset, gprs) in [
        (1, shaders::VERTEX_OFFSET, shaders::VERTEX_GPRS),
        (5, shaders::FRAGMENT_OFFSET, shaders::FRAGMENT_GPRS),
    ] {
        words.extend([packet(0x2000 + slot * 64, 2), 1 | (slot << 4), offset as u32]);
        words.extend([packet(0x200c + slot * 64, 1), gprs]);
    }
    // Reuse the engine-ordered epoch/nonce releases, omitting their duplicate bind.
    words.extend_from_slice(&channel::commands(target, epoch, nonce, true, reference)?[2..]);
    Ok(words)
}

pub struct Receipt {
    pub target: u64,
    pub epoch: u64,
    pub nonce: u32,
    pub before: [u32; 4],
    pub after: [u32; 4],
    pub observation: Option<channel::Observation>,
}
