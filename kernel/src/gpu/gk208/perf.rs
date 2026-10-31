/*
 * Copyright 2012 Nouveau Community
 * Copyright 2012 Red Hat Inc.
 * Copyright 2013 Red Hat Inc.
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
// claim-lint:ok: MIT grant; sources in docs/gui/gt710-clock-record.md.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
 * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 *
 * Authors: Martin Peres, Ben Skeggs
 */
//! Read-only BIT P / performance-v40 decoder; no register access or scripts.
//! MIT nouveau Linux v6.8 e8f897f4afef0031fe618a8e94127a0934896aba:
//! subdev/bios/bit.c:bit_entry; bios/perf.c:nvbios_perf_table, perf_entry,
//! perfEp, perfSp; clk/gk104.c:gk104_clk.domains; clk/base.c:nvkm_pstate_info.
use alloc::{format, string::String, vec::Vec};
use core::fmt::Write;

#[derive(Debug)]
pub struct Level {
    pub pstate: u8,
    pub gpc_khz: u32,
    pub core_khz: u32,
    pub memory_khz: u32,
}
#[derive(Debug)]
pub struct Table {
    pub bit_version: u8,
    pub offset: usize,
    pub raw: Vec<u8>,
    pub levels: Vec<Level>,
}
fn bytes(r: &[u8], p: usize, n: usize) -> Result<&[u8], &'static str> {
    r.get(p..p.checked_add(n).ok_or("table-overflow")?)
        .ok_or("table-outside-rom")
}
fn word(r: &[u8], p: usize) -> Result<usize, &'static str> {
    Ok(u16::from_le_bytes(bytes(r, p, 2)?.try_into().unwrap()) as usize)
}
pub fn decode(r: &[u8]) -> Result<Table, &'static str> {
    let bit = r
        .windows(6)
        .position(|v| v == b"\xff\xb8BIT\0")
        .ok_or("bit-missing")?;
    let h = bytes(r, bit, 12)?;
    if h[8] != 12 || h[9] != 6 || h[10] == 0 || h.iter().fold(0u8, |s, v| s.wrapping_add(*v)) != 0 {
        return Err("bit-header");
    }
    let entries = bytes(r, bit + 12, h[10] as usize * 6)?;
    let mut selected = None;
    for e in entries.chunks_exact(6).filter(|e| e[0] == b'P') {
        if selected.is_some() {
            return Err("duplicate-bit-p");
        }
        if !matches!(e[1], 1 | 2) || word(e, 2)? < 4 {
            return Err("bit-p-version-size");
        }
        let data = bytes(r, word(e, 4)?, word(e, 2)?)?;
        let offset = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
        if offset == 0 {
            return Err("performance-table-missing");
        }
        selected = Some((e[1], offset));
    }
    let (bit_version, offset) = selected.ok_or("bit-p-missing")?;
    let h = bytes(r, offset, 6)?;
    if h[0] != 0x40 {
        return Err("performance-version");
    }
    // v40: header, base-entry length, subentry size/count, entry count.
    let (header, base, size, count, levels) = (
        h[1] as usize,
        h[2] as usize,
        h[3] as usize,
        h[4] as usize,
        h[5] as usize,
    );
    if header < 6 || base < 12 || size < 2 || count < 4 || levels == 0 {
        return Err("performance-geometry");
    }
    let stride = base + size * count;
    let raw = bytes(r, offset, header + stride * levels)?;
    let mut decoded = Vec::new();
    for i in 0..levels {
        let p = header + i * stride;
        let pstate = raw[p];
        if pstate == 0xff {
            continue;
        } // nvkm_pstate_new ignores disabled entries.
        if decoded.iter().any(|l: &Level| l.pstate == pstate) {
            return Err("duplicate-pstate");
        }
        // gk104_clk.domains maps GPC to BIOS index 0 and memory to 3.
        let gpc_khz = (word(raw, p + base)? as u32 & 0x3fff) * 1000;
        let memory_khz = (word(raw, p + base + 3 * size)? as u32 & 0x3fff) * 1000;
        // nouveau displays core MHz with mdiv=2000. Memory here remains the
        // PLL/domain kHz (mdiv=500 is the source's effective DDR MHz display).
        decoded.push(Level {
            pstate,
            gpc_khz,
            core_khz: gpc_khz / 2,
            memory_khz,
        });
    }
    Ok(Table {
        bit_version,
        offset,
        raw: raw.to_vec(),
        levels: decoded,
    })
}
impl Table {
    /// Infer a level from a unique exact clock pair.
    pub fn current(
        &self,
        gpc: Result<u32, &'static str>,
        memory: Result<u32, &'static str>,
    ) -> (Option<u8>, &'static str) {
        let (Ok(gpc), Ok(memory)) = (gpc, memory) else {
            return (None, "clock-unavailable");
        };
        let mut matches = self
            .levels
            .iter()
            .filter(|l| l.gpc_khz == gpc && l.memory_khz == memory);
        let Some(level) = matches.next() else {
            return (None, "unmatched-clocks");
        };
        if matches.next().is_some() {
            return (None, "ambiguous-clocks");
        }
        (Some(level.pstate), "matched-clocks")
    }
}
pub fn record(
    table: Result<&Table, &'static str>,
    gpc: Result<u32, &'static str>,
    memory: Result<u32, &'static str>,
) -> String {
    let t = match table {
        Ok(t) => t,
        Err(reason) => return format!("{{\"status\":\"{reason}\"}}"),
    };
    let raw: String = t.raw.iter().map(|v| format!("{v:02x}")).collect();
    let mut levels = String::new();
    for l in &t.levels {
        if !levels.is_empty() {
            levels.push(',');
        }
        let _ = write!(
            levels,
            "{{\"pstate\":{},\"gpc_khz\":{},\"core_khz\":{},\"memory_khz\":{}}}",
            l.pstate, l.gpc_khz, l.core_khz, l.memory_khz
        );
    }
    let (pstate, status) = t.current(gpc, memory);
    let pstate = pstate.map_or_else(|| String::from("null"), |p| format!("{p}"));
    format!("{{\"status\":\"ok\",\"bit_version\":{},\"offset\":{},\"raw\":\"{}\",\"levels\":[{}],\"current\":{{\"pstate\":{},\"status\":\"{}\"}}}}", t.bit_version,t.offset,raw,levels,pstate,status)
}
