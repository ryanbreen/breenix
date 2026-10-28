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

//! A197 program binding only; drawing remains pending milestone 7.4.
// MIT cla197.h:2634-2638,3600-3619; Mesa nvc0_screen.c:895-898
// (upper/lower code address), nvc0_program.c:775-856 (SPH offsets).
use alloc::vec::Vec;
use super::{channel, shaders};

// MIT nouveau gf100.c:1414,1502,1614-1619: interrupt, trap and exception status.
pub const TRAP_REGISTERS: [usize; 4] = [0x400100, 0x400108, 0x400110, 0x400118];

/// No trap state is acknowledged or cleared by this probe.
pub fn check_traps(words: [u32; 4]) -> Result<(), &'static str> {
    if words != [0; 4] { return Err("shader binding PGRAPH trap/exception"); }
    Ok(())
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
