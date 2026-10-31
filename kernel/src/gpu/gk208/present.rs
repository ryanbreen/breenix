/*
 * SPDX-FileCopyrightText: Copyright (c) 1993-2022 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
 * SPDX-License-Identifier: MIT
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
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
 * THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
 * FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
 * DEALINGS IN THE SOFTWARE.
 */
/*
 * Copyright 2018 Red Hat Inc.
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
//! NV927D surface updates. Source/units: MIT cl927d.h, nouveau head907d.c,
//! wndw.c and usergf119.c; exact revisions in THIRD_PARTY_NOTICES.
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Surface {
    pub resource: u32,
    pub address: u64,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    /// Value 0 denotes pitch-linear console storage; otherwise height in GOBs.
    pub block_height: u32,
    pub format: u32,
    pub dma: u32,
}
impl Surface {
    pub fn methods(self, head: u32) -> Result<Vec<(u32,u32)>, &'static str> {
        if head >= 4 || self.resource == 0 || self.address & 255 != 0
            || self.address >= 1 << 40 || self.width == 0 || self.height == 0
            || self.width > 16384 || self.height > 16384 || self.pitch < self.width * 4
            || self.pitch % if self.block_height==0 {256} else {64} != 0 || self.dma == 0 {
            return Err("invalid native scanout surface");
        }
        // wndw.c:296-308: block-linear pitch is a count of 64-byte blocks.
        let storage = if self.block_height == 0 { 0x01000000 | self.pitch }
            else if self.block_height.is_power_of_two() && self.block_height <= 32 {
                ((self.pitch / 64) << 8) | self.block_height.trailing_zeros()
            } else { return Err("invalid native scanout block height"); };
        if storage & 0x00e00000 != 0 { return Err("native scanout pitch overflow"); }
        let params = match self.format { 0 => 0xcf00, 1 => 0xd500,
            _ => return Err("unsupported native scanout format") };
        let h=head*0x300;
        let mut methods = Vec::new();
        methods.try_reserve_exact(7).map_err(|_| "native surface method allocation")?;
        methods.extend([(0x460+h,(self.address>>8) as u32),
            (0x468+h,self.width | self.height<<16),(0x46c+h,storage),
            (0x470+h,params),(0x474+h,self.dma),(0x84,0x80000000),(0x80,0)]);
        Ok(methods)
    }
    /// GF119 display DMA: physical VRAM, small pages, uncompressed color kind
    /// 0xfe for tiled targets; kind 0 for the pitch-linear console. Absolute
    /// HEAD_SET_OFFSET is bounded by this retained whole-VRAM descriptor.
    pub fn dma_words(self, total: u64) -> Result<[u32;6], &'static str> {
        if total == 0 || total > 1 << 40 || total & 255 != 0 {
            return Err("invalid scanout DMA extent");
        }
        Ok([0x49 | if self.block_height == 0 {0} else {0xfe << 20},
            0,((total-1)>>8) as u32,0,0,0])
    }
}
pub struct Receipt { pub notifier:u32, pub active:[u32;5], pub armed:[u32;5] }
pub trait Io {
    fn clear_notifier(&mut self) -> Result<(), &'static str>;
    fn submit(&mut self, methods:&[(u32,u32)]) -> Result<(), &'static str>;
    fn complete(&mut self) -> Result<u32, &'static str>;
    fn read(&mut self, register:usize) -> u32;
}
/// A completed update must name the same surface in both the active and
/// assembly state, including size, tiling, pixel format and context DMA.
/// gf119.c's head_state and core method table establish the two register banks.
pub fn flush(io: &mut impl Io, head:u32, surface:Surface) -> Result<Receipt, &'static str> {
    let methods=surface.methods(head)?;
    io.clear_notifier()?;
    io.submit(&methods)?;
    let notifier=io.complete()?;
    receipt(io, head, surface, notifier)
}

/// An already latched surface needs no new head state. Its content release is
/// still followed by a fresh EVO acknowledgement and both-bank observations.
pub fn retained<T: Io>(io: &mut T, head: u32, surface: Surface,
    acknowledge: impl FnOnce(&mut T) -> Result<u32, &'static str>,
) -> Result<Option<Receipt>, &'static str> {
    if surface_state(io, head, surface).is_err() { return Ok(None); }
    let notifier = acknowledge(io)?;
    receipt(io, head, surface, notifier).map(Some)
}

fn receipt(io: &mut impl Io, head: u32, surface: Surface, notifier: u32)
    -> Result<Receipt, &'static str> {
    if notifier & 1 != 1 { return Err("native present update not complete"); }
    let (active, armed) = surface_state(io, head, surface)?;
    Ok(Receipt{notifier,active,armed})
}

fn surface_state(io: &mut impl Io, head: u32, surface: Surface)
    -> Result<([u32; 5], [u32; 5]), &'static str> {
    let methods = surface.methods(head)?;
    let mut active=[0;5];let mut armed=[0;5];
    for (i,&(method,expected)) in methods[..5].iter().enumerate() {
        active[i]=io.read(0x640000+method as usize);
        armed[i]=io.read(0x660000+method as usize);
        if active[i] != expected || armed[i] != expected {
            return Err("native present latched a different surface");
        }
    }
    Ok((active,armed))
}
