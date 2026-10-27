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
//! Per-directory PAGE_ALL invalidation, adapted from the checked MIT
//! nouveau v6.8 vmmgf100.c; hardware waits/drains use the existing BAR layer.
use super::modeset::{self, Io};
pub fn invalidate(a: &mut impl Io, directory: u64) -> Result<u32, &'static str> {
    if directory % 4096 != 0 || directory >= 1 << 40 {
        return Err("invalid GPU directory address");
    }
    modeset::bar_flush(a)?;
    let mut free = false;
    for _ in 0..20000 {
        if a.read(0x100c80) & 0x00ff0000 != 0 {
            free = true;
            break;
        }
        a.delay_us(100);
    }
    if !free {
        return Err("GPU invalidation queue has no free slot");
    }
    a.write(0x100cb8, (directory >> 8) as u32);
    a.write(0x100cbc, 0x80000001);
    modeset::wait(a, 0x100c80, 0x8000, 0x8000)?;
    Ok(a.read(0x100c80))
}
