/*
 * Copyright 2012 Red Hat Inc.
 * Copyright 2013 Red Hat Inc.
 * Copyright 2014 Martin Peres
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
 * claim-lint:ok: MIT license condition; source paths in docs/gui/gt710-clock-record.md.
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
//! Read-only GK104 clock and thermal observations (G1, M8 PR 8.7).
//! Adapted from MIT nouveau at Linux v6.8 e8f897f4afef0031fe618a8e94127a0934896aba:
//! clk/gk104.c:46-220, engine/device/base.c:3344-3359,
//! therm/g84.c:29-38 and fuse/gf100.c:27-45. Full paths in THIRD_PARTY_NOTICES.
//! Host means nv_clk_src_href (host reference), not an inferred hub clock.
use alloc::vec::Vec;

pub struct Sample {
    pub raw: Vec<(u32, u32)>,
    pub gpc_khz: Result<u32, &'static str>,
    pub memory_khz: Result<u32, &'static str>,
    pub host_khz: Result<u32, &'static str>,
    pub temperature_c: Result<u32, &'static str>,
}

// Memoize each word: the raw evidence and the decoder see the same observation.
struct Reader<F> { read: F, raw: Vec<(u32, u32)>, crystal: u32 }
impl<F: FnMut(u32) -> u32> Reader<F> {
    fn reg(&mut self, address: u32) -> u32 {
        if let Some((_, value)) = self.raw.iter().find(|(a, _)| *a == address) { return *value; }
        let value = (self.read)(address);
        self.raw.push((address, value));
        value
    }
    fn vco(&mut self, source: u32) -> Result<u32, &'static str> {
        let pll = if self.reg(source) & 0x100 == 0 { 0xe800 } else { 0xe820 };
        self.pll(pll)
    }
    fn pll(&mut self, address: u32) -> Result<u32, &'static str> {
        if !matches!(address, 0xe800 | 0xe820 | 0x132000 | 0x132020 | 0x137000) {
            return Err("unknown-pll");
        }
        let ctrl = self.reg(address);
        let coef = self.reg(address + 4);
        if ctrl & 1 == 0 { return Err("pll-disabled"); }
        let mut p = (coef >> 16) & 0x3f;
        let n = (coef >> 8) & 0xff;
        let m = coef & 0xff;
        if m == 0 { return Err("zero-pll-divisor"); }
        let mut fractional = 0xf000u16;
        let source = match address {
            0xe800 | 0xe820 => { p = 1; self.crystal }
            0x132000 => { p = if coef & 0x10000000 != 0 { 2 } else { 1 }; self.pll(0x132020)? }
            0x132020 => {
                let source = self.div(0, 0x137320, 0x137330)?;
                fractional = (self.reg(address + 0x10) >> 16) as u16;
                source
            }
            0x137000 => self.div(0, 0x137120, 0x137140)?,
            _ => return Err("unknown-pll"),
        };
        if p == 0 { p = 1; }
        // Match nouveau's u16 wrap before the fractional multiply and its
        // integer truncation at each divider. u64 avoids overflowing the host.
        let numerator = u64::from(source) * u64::from(n)
            + ((u64::from(fractional.wrapping_add(4096)) * u64::from(source)) >> 13);
        u32::try_from(numerator / u64::from(m * p)).map_err(|_| "clock-overflow")
    }
    fn div(&mut self, index: u32, source: u32, control: u32) -> Result<u32, &'static str> {
        let source_address = source + index * 4;
        let src = self.reg(source_address);
        let ctl = self.reg(control + index * 4);
        match src & 3 {
            0 => Ok(if src & 0x30000 == 0x30000 { 108000 } else { self.crystal }),
            2 => Ok(100000),
            3 => {
                let frequency = self.vco(source_address)?;
                Ok(if ctl & 0x80000000 != 0 { frequency * 2 / ((ctl & 0x3f) + 2) } else { frequency })
            }
            _ => Err("unknown-divider-source"),
        }
    }
    fn gpc(&mut self) -> Result<u32, &'static str> {
        let control = self.reg(0x137250);
        let pll = self.reg(0x137100) & 1 != 0;
        let frequency = if pll { self.pll(0x137000)? } else { self.div(0, 0x137160, 0x1371d0)? };
        Ok(if control & 0x80000000 != 0 {
            let div = if pll { (control >> 8) & 0x3f } else { control & 0x3f };
            frequency * 2 / (div + 2)
        } else { frequency })
    }
    fn memory(&mut self) -> Result<u32, &'static str> {
        match self.reg(0x1373f4) & 15 {
            1 => self.pll(0x132020),
            2 => self.pll(0x132000),
            _ => Err("unknown-memory-mode"),
        }
    }
    fn temperature(&mut self) -> Result<u32, &'static str> {
        // gf100_fuse_read enables these bits before reading 0x21100+0x1a8.
        // Observe only: if POST did not leave both set, do not touch the fuse
        // aperture or run g84_sensor_setup. No gate or sensor writes are needed.
        let enable = self.reg(0x22400);
        let access = self.reg(0x21000);
        if enable & 0x800 == 0 || access & 1 == 0 { return Err("fuse-access-disabled"); }
        if self.reg(0x212a8) != 1 { return Err("sensor-unavailable"); }
        let value = self.reg(0x20400);
        if value > i32::MAX as u32 { return Err("sensor-read-error"); }
        Ok(value)
    }
}

pub fn sample(read: impl FnMut(u32) -> u32) -> Sample {
    let mut reader = Reader { read, raw: Vec::new(), crystal: 0 };
    reader.crystal = match reader.reg(0x101000) & 0x400040 {
        0 => 13500, 0x40 => 14318, 0x400000 => 27000, _ => 25000,
    };
    let gpc_khz = reader.gpc();
    let memory_khz = reader.memory();
    let temperature_c = reader.temperature();
    Sample { raw: reader.raw, gpc_khz, memory_khz, host_khz: Ok(100000), temperature_c }
}

#[cfg(target_os = "none")]
mod runtime {
    use super::*;
    use alloc::{format, string::String};
    use core::sync::atomic::{AtomicUsize, Ordering};
    static BAR: AtomicUsize = AtomicUsize::new(0);
    static SLOT: AtomicUsize = AtomicUsize::new(0);

    pub fn initialize(base: usize) { BAR.store(base, Ordering::Release); }
    fn reading(value: Result<u32, &'static str>) -> String {
        match value {
            Ok(value) => format!("{{\"value\":{},\"status\":\"ok\"}}", value),
            Err(reason) => format!("{{\"value\":null,\"status\":\"{}\"}}", reason),
        }
    }
    pub fn export() -> Option<(String, String)> {
        let base = BAR.load(Ordering::Acquire);
        if base == 0 { return None; }
        let phase = match SLOT.fetch_add(1, Ordering::Relaxed) { 0 => "startup", 1 => "final", _ => return None };
        let sample = sample(|address| {
            // SAFETY: inventory retains the checked uncached 16 MiB BAR0
            // mapping; source-defined offsets above lie inside it. Reads only.
            unsafe { core::ptr::read_volatile((base + address as usize) as *const u32) }
        });
        let raw: Vec<_> = sample.raw.iter().map(|(a, v)| format!("[{},{}]", a, v)).collect();
        let row = format!("GT710_CLOCK_V1 {{\"schema\":1,\"phase\":\"{}\",\"host_source\":\"href\",\"raw\":[{}],\"gpc_khz\":{},\"memory_khz\":{},\"host_khz\":{},\"temperature_c\":{}}}",
            phase, raw.join(","), reading(sample.gpc_khz), reading(sample.memory_khz), reading(sample.host_khz), reading(sample.temperature_c));
        let anchor = format!("GT710_CLOCK_BULK phase={} bytes={} sha256={}", phase, row.len(), super::super::hash(row.as_bytes()));
        Some((row, anchor))
    }
}
#[cfg(target_os = "none")]
pub use runtime::{initialize, export};
