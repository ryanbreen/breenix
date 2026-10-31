/*
 * Copyright (c) 2006 Luc Verhaegen (quirks list)
 * Copyright (c) 2007-2008 Intel Corporation
 *   Jesse Barnes <jesse.barnes@intel.com>
 * Copyright 2010 Red Hat, Inc.
 *
 * DDC probing routines (drm_ddc_read & drm_do_probe_ddc_edid) originally from
 * FB layer.
 *   Copyright (C) 2006 Dennis Munsie <dmunsie@cecropia.com>
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sub license,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice (including the
 * next paragraph) shall be included in all copies or substantial portions
 * of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NON-INFRINGEMENT. IN NO EVENT SHALL
 * THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
 * FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
 * DEALINGS IN THE SOFTWARE.
 */
//! VESA E-EDID A2 detailed timings and the required DMT standard timing.
extern crate alloc;
use alloc::vec::Vec;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub width: u32,
    pub height: u32,
    pub clock_khz: u32,
    pub hsync_start: u32,
    pub hsync_end: u32,
    pub htotal: u32,
    pub vsync_start: u32,
    pub vsync_end: u32,
    pub vtotal: u32,
    pub flags: u8,
}
fn detailed(d: &[u8], modes: &mut Vec<Timing>) -> Result<(), &'static str> {
    let clock_khz = u16::from_le_bytes([d[0], d[1]]) as u32 * 10;
    if clock_khz == 0 {
        return Ok(());
    }
    let width = d[2] as u32 + ((d[4] as u32 & 0xf0) << 4);
    let height = d[5] as u32 + ((d[7] as u32 & 0xf0) << 4);
    let hb = d[3] as u32 + ((d[4] as u32 & 15) << 8);
    let vb = d[6] as u32 + ((d[7] as u32 & 15) << 8);
    let hs = d[8] as u32 + ((d[11] as u32 & 0xc0) << 2);
    let hw = d[9] as u32 + ((d[11] as u32 & 0x30) << 4);
    let vs = (d[10] >> 4) as u32 + ((d[11] as u32 & 12) << 2);
    let vw = (d[10] & 15) as u32 + ((d[11] as u32 & 3) << 4);
    if width == 0
        || height == 0
        || hs == 0
        || hw == 0
        || vs == 0
        || vw == 0
        || hs + hw > hb
        || vs + vw > vb
    {
        return Err("invalid detailed timing");
    }
    if d[17] & 0x98 != 0x18 {
        return Ok(());
    } // progressive digital separate sync
    let t = Timing {
        width,
        height,
        clock_khz,
        hsync_start: width + hs,
        hsync_end: width + hs + hw,
        htotal: width + hb,
        vsync_start: height + vs,
        vsync_end: height + vs + vw,
        vtotal: height + vb,
        flags: d[17],
    };
    if !modes.contains(&t) {
        modes.push(t);
    }
    Ok(())
}
pub fn parse(e: &[u8]) -> Result<Vec<Timing>, &'static str> {
    if e.len() < 128
        || e[..8] != [0, 255, 255, 255, 255, 255, 255, 0]
        || e[18] != 1
        || !(3..=4).contains(&e[19])
        || e[20] & 128 == 0
    {
        return Err("EDID header/version/digital input");
    }
    if e[126] > 3
        || e.len() != (e[126] as usize + 1) * 128
        || e.chunks_exact(128)
            .any(|b| b.iter().fold(0u8, |a, v| a.wrapping_add(*v)) != 0)
    {
        return Err("EDID length/checksum");
    }
    let mut modes = Vec::new();
    for d in e[54..126].chunks_exact(18) {
        detailed(d, &mut modes)?;
    }
    // DMT 1Ch, VESA DMT 1.0 rev 13 p.46; standard code 81 00.
    if e[38..54].chunks_exact(2).any(|s| s == [0x81, 0]) {
        let low = Timing {
            width: 1280,
            height: 800,
            clock_khz: 83500,
            hsync_start: 1352,
            hsync_end: 1480,
            htotal: 1680,
            vsync_start: 803,
            vsync_end: 809,
            vtotal: 831,
            flags: 28,
        };
        if !modes.contains(&low) {
            modes.push(low);
        }
    }
    // E-EDID A2 table 3.18 p.30; DMT 1.13 p.33 (ID 10h).
    if e[36] & 8 != 0 || e[38..54].chunks_exact(2).any(|s| s == [0x61, 0x40]) {
        let low = Timing { width:1024,height:768,clock_khz:65000,
            hsync_start:1048,hsync_end:1184,htotal:1344,
            vsync_start:771,vsync_end:777,vtotal:806,flags:24 };
        if !modes.contains(&low) { modes.push(low); }
    }
    // E-EDID A2 table 3.18: byte 36 bit 0; VESA DMT 1.13 ID 24h.
    // The tuple is also in MIT drm_edid.c (Linux v6.8), drm_dmt_modes
    // and edid_est_modes; the original permission notice is above.
    if e[36] & 1 != 0 && !modes.contains(&DMT_1280_1024_75) {
        modes.push(DMT_1280_1024_75);
    }
    for b in e[128..].chunks_exact(128) {
        if b[0] == 2 && b[2] != 0 {
            let start = b[2] as usize;
            if !(4..=127).contains(&start) {
                return Err("CTA detailed timing offset");
            }
            for d in b[start..127].chunks_exact(18) {
                detailed(d, &mut modes)?;
            }
        }
    }
    Ok(modes)
}

const DMT_1280_1024_75: Timing = Timing {
    width: 1280, height: 1024, clock_khz: 135000,
    hsync_start: 1296, hsync_end: 1440, htotal: 1688,
    vsync_start: 1025, vsync_end: 1028, vtotal: 1066, flags: 30,
};

/// Select the sink's preferred detailed timing for the physical raster (G3/G20/G24).
pub fn native_raster(modes: &[Timing]) -> Result<&Timing, &'static str> {
    modes.first().ok_or("no preferred sink timing")
}
