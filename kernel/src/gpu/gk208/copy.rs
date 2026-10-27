/*******************************************************************************
    Copyright (c) 2019, NVIDIA CORPORATION. All rights reserved.

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

//! Kernel-only virtual A0B5 pitch copies. Overlap is staged by the caller.
use alloc::{vec, vec::Vec};
#[derive(Clone, Copy)]
pub struct Rect {
    pub src: u64,
    pub dst: u64,
    pub width: u32,
    pub height: u32,
    pub src_pitch: u32,
    pub dst_pitch: u32,
}
fn end(a: u64, pitch: u32, width: u32, height: u32) -> Result<u64, &'static str> {
    if width == 0 || height == 0 || width > pitch || height > 65536 {
        return Err("invalid CE rectangle");
    }
    let e = a.checked_add(u64::from(height - 1) * u64::from(pitch))
        .and_then(|v| v.checked_add(u64::from(width))).ok_or("CE extent overflow")?;
    if e > 1 << 40 { return Err("CE extent exceeds GPU VA"); }
    Ok(e)
}
fn method(out: &mut Vec<u32>, address: u32, values: &[u32]) {
    out.push(0x20000000 | ((values.len() as u32) << 16) | (4 << 13) | (address >> 2));
    out.extend_from_slice(values);
}
/// The three completion words are engine releases, following a non-pipelined
/// flushed transfer; a PFIFO front-end release cannot certify copy completion.
pub fn encode(r: Rect, semaphore: u64, epoch: u64, sequence: u32) -> Result<Vec<u32>, &'static str> {
    let s = end(r.src, r.src_pitch, r.width, r.height)?;
    let d = end(r.dst, r.dst_pitch, r.width, r.height)?;
    if r.src < d && r.dst < s { return Err("CE overlap requires staging"); }
    if semaphore % 4 != 0 || semaphore > (1 << 40) - 12 || epoch == 0 {
        return Err("invalid CE completion record");
    }
    let mut out = vec![];
    method(&mut out, 0, &[0xa0b5]);
    method(&mut out, 0x400, &[(r.src >> 32) as u32, r.src as u32,
        (r.dst >> 32) as u32, r.dst as u32, r.src_pitch, r.dst_pitch, r.width, r.height]);
    method(&mut out, 0x300, &[0x386]);
    for (offset, value) in [(4, epoch as u32), (8, (epoch >> 32) as u32), (0, sequence)] {
        let address = semaphore + offset;
        method(&mut out, 0x240, &[(address >> 32) as u32, address as u32, value]);
        method(&mut out, 0x300, &[0x0c]);
    }
    Ok(out)
}
