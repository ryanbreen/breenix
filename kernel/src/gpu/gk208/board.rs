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
/*
 * Copyright 2005-2006 Erik Waling
 * Copyright 2006 Stephane Marchesin
 * Copyright 2007-2009 Stuart Bennett
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
 * THE AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
 * WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF
 * OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */
//! Bounded legacy VBIOS inventory; no script execution or ROM redistribution.
extern crate alloc;
use alloc::vec::Vec;

#[derive(Debug)]
pub struct Board {
    pub bit_version: u16,
    pub dcb_version: u8,
    pub i2c_version: u8,
    pub connector_version: u8,
    pub pll_version: u8,
    pub routes: Vec<Route>,
}
#[derive(Debug)]
pub struct Route {
    pub entry: u32,
    pub conf: u32,
    pub i2c_entry: u32,
    pub connector: u8,
    pub kind: u8,
    pub drive: u8,
    pub sor: u8,
    pub heads: u8,
}
fn bytes(r: &[u8], p: usize, n: usize) -> Result<&[u8], &'static str> {
    r.get(p..p.checked_add(n).ok_or("table overflow")?)
        .ok_or("table outside ROM")
}
fn u16at(r: &[u8], p: usize) -> Result<usize, &'static str> {
    Ok(u16::from_le_bytes(bytes(r, p, 2)?.try_into().unwrap()) as usize)
}
fn u32at(r: &[u8], p: usize) -> Result<u32, &'static str> {
    Ok(u32::from_le_bytes(bytes(r, p, 4)?.try_into().unwrap()))
}
fn sum(r: &[u8]) -> u8 {
    r.iter().fold(0u8, |a, b| a.wrapping_add(*b))
}
fn table(
    r: &[u8],
    p: usize,
    min_header: usize,
    min_entry: usize,
) -> Result<(usize, usize, usize), &'static str> {
    let h = bytes(r, p, 4)?;
    let (header, count, stride) = (h[1] as usize, h[2] as usize, h[3] as usize);
    if p == 0 || header < min_header || stride < min_entry || count == 0 || count > 64 {
        return Err("invalid table geometry");
    }
    bytes(r, p, header + count * stride)?;
    Ok((p + header, count, stride))
}
pub fn parse(r: &[u8]) -> Result<Board, &'static str> {
    if bytes(r, 0, 3)?[..2] != [0x55, 0xaa] {
        return Err("ROM signature");
    }
    let pcir = u16at(r, 0x18)?;
    if bytes(r, pcir, 8)? != b"PCIR\xde\x10\x8b\x12" {
        return Err("ROM PCI identity");
    }
    let len = u16at(r, pcir + 0x10)? * 512;
    if len == 0 || len != r.len() || sum(r) != 0 || r[2] as usize * 512 > len {
        return Err("ROM length/checksum");
    }
    let bit = r
        .windows(6)
        .position(|x| x == b"\xff\xb8BIT\0")
        .ok_or("BIT missing")?;
    let bh = bytes(r, bit, 12)?;
    if bh[8] != 12 || bh[9] != 6 || bh[10] == 0 || sum(bh) != 0 {
        return Err("BIT header");
    }
    let entries = bytes(r, bit + 12, bh[10] as usize * 6)?;
    let mut pll = None;
    for e in entries.chunks_exact(6) {
        let size = u16::from_le_bytes([e[2], e[3]]) as usize;
        let p = u16::from_le_bytes([e[4], e[5]]) as usize;
        bytes(r, p, size)?;
        if e[0] == b'C' {
            let target = match e[1] {
                1 if size >= 10 => u16at(r, p + 8)?,
                2 if size == 4 => u32at(r, p)? as usize,
                _ => return Err("BIT C version/size"),
            };
            let h = bytes(r, target, 4)?;
            // PLL tables put record length before count, unlike DCB tables.
            if !matches!(h[0], 0x30 | 0x40 | 0x50) || h[1] < 4 || h[2] < 7 || h[3] == 0 {
                return Err("PLL header");
            }
            bytes(r, target, h[1] as usize + h[2] as usize * h[3] as usize)?;
            pll = Some(h[0]);
        }
    }
    let dcb = u16at(r, 0x36)?;
    let (ds, dc, dl) = table(r, dcb, 0x16, 8)?;
    if !matches!(r[dcb], 0x40 | 0x41) || u32at(r, dcb + 6)? != 0x4edcbdcb {
        return Err("DCB signature/version");
    }
    let i2c = u16at(r, dcb + 4)?;
    let (is, ic, il) = table(r, i2c, 4, 4)?;
    if r[i2c] != 0x41 {
        return Err("unsupported I2C table");
    }
    let conn = u16at(r, dcb + 0x14)?;
    let (cs, cc, cl) = table(r, conn, 4, 4)?;
    if !matches!(r[conn], 0x30 | 0x40) {
        return Err("connector table version");
    }
    let mut routes = Vec::new();
    for i in 0..dc {
        let entry = u32at(r, ds + i * dl)?;
        let conf = u32at(r, ds + i * dl + 4)?;
        if entry == 0 || entry == u32::MAX || entry & 15 == 15 {
            break;
        }
        if entry & 15 != 2 || entry & 0x00300000 != 0 {
            continue;
        } // on-board TMDS only
        let connector = ((entry >> 12) & 15) as usize;
        let index = ((entry >> 4) & 15) as usize;
        if connector >= cc || index >= ic {
            return Err("route table index");
        }
        let i2c_entry = u32at(r, is + index * il)?;
        let drive = (i2c_entry & 31) as u8;
        let sor_mask = (entry >> 24) & 15;
        if drive == 31 {
            continue;
        }
        if sor_mask.count_ones() != 1 || entry & 0xf00 == 0 {
            return Err("ambiguous SOR/head route");
        }
        routes.push(Route {
            entry,
            conf,
            i2c_entry,
            connector: connector as u8,
            kind: r[cs + connector * cl],
            drive,
            sor: sor_mask.trailing_zeros() as u8,
            heads: ((entry >> 8) & 15) as u8,
        });
    }
    if routes.is_empty() {
        return Err("no board-routed TMDS connector");
    }
    Ok(Board {
        bit_version: u16at(r, bit + 6)? as u16,
        dcb_version: r[dcb],
        i2c_version: r[i2c],
        connector_version: r[conn],
        pll_version: pll.ok_or("PLL missing")?,
        routes,
    })
}
