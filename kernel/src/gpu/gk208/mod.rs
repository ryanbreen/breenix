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
 * Copyright 2015 Red Hat Inc.
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
 * Authors: Ben Skeggs <bskeggs@redhat.com>
 */
/*
 * Copyright 2014 Red Hat Inc.
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
//! GK208B board inventory, before native display ownership (E19 PR 4.1).
//! Runs once on the boot CPU before secondary CPUs and GPU clients exist.
pub mod gr;
pub use gr::tables as gr_tables;
mod board;
mod ddc;
mod edid;
mod bulk;
mod memory;
mod inventory;
mod vm;
mod lifetime;
mod vm_control;
mod vm_probe;
mod aperture;
mod dma_policy;
mod system_dma;
mod aperture_probe;
mod channel;
mod copy;
mod ce;
mod queue;
mod recovery_policy;
pub use system_dma::set_platform as set_dma_platform;
mod modeset;
mod native;
pub use native::{ConsoleSurface,console_binding,takeover_started};
mod rom;
pub fn proc_pixels()->Vec<u8> {bulk::take()}
pub fn proc_ce(op:u8)->Result<Vec<u8>,i32> {ce::proc_read(op)}
use crate::arch_impl::x86_64::timer;
use crate::drivers::pci;
use alloc::{format, string::String, vec::Vec};
use core::fmt::Write;

struct Registers(usize);
impl Registers {
    fn read(&self, reg: usize) -> u32 {
        // SAFETY: offsets used here fit the checked, uncached 16 MiB BAR0 mapping.
        unsafe { ((self.0 + reg) as *const u32).read_volatile() }
    }
    fn write(&self, reg: usize, value: u32) {
        // SAFETY: checked BAR0 mapping; board access and native display/MMU bootstrap.
        unsafe {
            ((self.0 + reg) as *mut u32).write_volatile(value);
        }
        let _ = self.read(reg); // drain the posted MMIO write
    }
    fn mask(&self, reg: usize, mask: u32, value: u32) {
        self.write(reg, (self.read(reg) & !mask) | value);
    }
}
fn hex(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        let _ = write!(s, "{:02x}", b);
    }
    s
}
fn hash(data: &[u8]) -> String {
    hex(&crate::block::install::sha256::sha256(data))
}

impl memory::Access for Registers {
    fn read(&mut self, reg: usize) -> u32 {
        Registers::read(self, reg)
    }
    fn write(&mut self, reg: usize, value: u32) {
        Registers::write(self, reg, value)
    }
}
impl rom::Access for Registers {
    fn read(&self, reg: usize) -> u32 {
        Registers::read(self, reg)
    }
    fn write(&mut self, reg: usize, value: u32) {
        Registers::write(self, reg, value);
    }
}
struct Bus<'a> {
    regs: &'a Registers,
    reg: usize,
    saved: u32,
    pad: Option<(usize, u32, u32)>,
    ticks: u64,
}
impl<'a> Bus<'a> {
    fn new(regs: &'a Registers, route: &board::Route) -> Result<Self, &'static str> {
        let frequency = timer::frequency_hz();
        if frequency < 1_000_000 {
            return Err("DDC requires calibrated TSC");
        }
        let aux = route.share as usize;
        let pad = if aux != 31 {
            if aux >= 4 {
                return Err("unsupported shared DDC pad");
            }
            let p = 0xe500 + aux * 0x50;
            let saved = (p, regs.read(p), regs.read(p + 12));
            regs.mask(p, 0xc003, 0xc001);
            regs.mask(p + 12, 1, 0);
            Some(saved)
        } else {
            None
        };
        let reg = 0xd014 + route.drive as usize * 0x20;
        let saved = regs.read(reg) & 7;
        regs.write(reg, 7);
        Ok(Self {
            regs,
            reg,
            saved,
            pad,
            ticks: frequency / 100_000,
        }) // 10 us per half-cycle
    }
}
impl ddc::Pins for Bus<'_> {
    fn drive(&mut self, scl: bool, sda: bool) {
        self.regs
            .mask(self.reg, 3, u32::from(scl) | (u32::from(sda) << 1));
    }
    fn sense(&mut self) -> (bool, bool) {
        let value = self.regs.read(self.reg);
        (value & 16 != 0, value & 32 != 0)
    }
    fn delay(&mut self) {
        let begin = timer::rdtsc();
        while timer::rdtsc().wrapping_sub(begin) < self.ticks {
            core::hint::spin_loop();
        }
    }
}
impl Drop for Bus<'_> {
    fn drop(&mut self) {
        self.regs.write(self.reg, self.saved);
        if let Some((p, mode, power)) = self.pad {
            self.regs.mask(p, 0xc003, mode & 0xc003);
            self.regs.mask(p + 12, 1, power & 1);
        }
    }
}
fn sink(
    regs: &Registers,
    route: &board::Route,
) -> Result<(Vec<u8>, Vec<edid::Timing>), &'static str> {
    let mut bus = Bus::new(regs, route)?;
    let mut block = [0u8; 128];
    ddc::read_block(&mut bus, 0, &mut block)?;
    if block[126] > 3 {
        return Err("EDID extensions exceed supported bound");
    }
    let mut data = block.to_vec();
    for n in 1..=block[126] {
        let mut next = [0; 128];
        ddc::read_block(&mut bus, n, &mut next)?;
        data.extend_from_slice(&next);
    }
    let modes = edid::parse(&data)?;
    Ok((data, modes))
}
fn inventory(device: pci::Device) -> Result<ConsoleSurface, &'static str> {
    let bar = device.bars[0];
    if bar.address == 0 || bar.size < 0x1000000 || bar.is_io || bar.prefetchable {
        return Err("BAR0 is not an assigned register aperture");
    }
    let mut regs = Registers(crate::memory::map_mmio(bar.address, 0x1000000)?);
    let boot0 = regs.read(0);
    let chipset = (boot0 >> 20) & 0x1ff;
    let post = regs.read(0x2240c);
    crate::serial_println!(
        "GT710_PROBE boot0={:#x} chipset={:#x} post={:#x}",
        boot0,
        chipset,
        post
    );
    if chipset != 0x106 || post & 2 == 0 {
        return Err("GK208B must be POSTed before inventory");
    }
    let rom = rom::read(&mut regs)?;
    let board = board::parse(&rom)?;
    for route in &board.routes {
        let state = regs.read(0x640200 + route.sor as usize * 0x20);
        if !matches!((state >> 8) & 15, 1 | 2 | 5)
            || state & 15 == 0
            || state & 15 & !(route.heads as u32) != 0
        {
            continue;
        }
        let head = (state & 15).trailing_zeros();
        if (state & 15).count_ones() != 1 {
            return Err("ambiguous active TMDS head");
        }
        let (data, modes) = match sink(&regs, route) {
            Ok(s) => s,
            Err(e) => {
                crate::serial_println!(
                    "GT710_PROBE connector={} drive={} error={}",
                    route.connector,
                    route.drive,
                    e
                );
                continue;
            }
        };
        let mut timings = String::new();
        for t in &modes {
            if !timings.is_empty() {
                timings.push(',');
            }
            let _=write!(timings,"{{\"width\":{},\"height\":{},\"clock_khz\":{},\"hsync_start\":{},\"hsync_end\":{},\"htotal\":{},\"vsync_start\":{},\"vsync_end\":{},\"vtotal\":{},\"flags\":{}}}",t.width,t.height,t.clock_khz,t.hsync_start,t.hsync_end,t.htotal,t.vsync_start,t.vsync_end,t.vtotal,t.flags);
        }
        let evidence=format!(concat!("{{\"schema\":1,\"phase\":\"board\",\"bdf\":\"{:02x}:{:02x}.{}\",",
            "\"boot0\":{},\"chipset\":{},\"revision\":{},\"post\":{},\"rom_bytes\":{},\"rom_sha256\":\"{}\",",
            "\"bit_version\":{},\"dcb_version\":{},\"i2c_version\":{},\"connector_version\":{},\"pll_version\":{},",
            "\"dcb_entry\":{},\"dcb_conf\":{},\"i2c_entry\":{},\"connector\":{},\"connector_type\":{},\"drive\":{},\"sor\":{},",
            "\"sor_state\":{},\"head\":{},\"head_total\":{},\"head_clock\":{},\"ddc\":\"live\",",
            "\"edid\":\"{}\",\"edid_sha256\":\"{}\",\"modes\":[{}]}}"),
            device.bus,device.device,device.function,boot0,chipset,boot0&255,post,rom.len(),hash(&rom),
            board.bit_version,board.dcb_version,board.i2c_version,board.connector_version,board.pll_version,
            route.entry,route.conf,route.i2c_entry,route.connector,route.kind,route.drive,route.sor,state,head,
            regs.read(0x640414+head as usize*0x300),regs.read(0x640450+head as usize*0x300),hex(&data),hash(&data),timings);
        crate::serial_println!("GT710_NATIVE {}", evidence);
        let preferred = modes.first().ok_or("no preferred sink timing")?;
        let mut layout=display_memory(
            &mut regs,
            &device,
            preferred.width.into(),
            preferred.height.into(),
        )?;
        let result=native::takeover(&mut regs,&device,&rom,&modes,route,head,&layout);
        if result.is_err() {
            crate::serial_println!("GT710_MODE_STATE ctrl={:#x} error={:#x} stat={:#x} data={:#x} code={:#x} put={} get={} super={:#x} head-super={:#x}",
                regs.read(0x610490),regs.read(0x61009c),regs.read(0x6101f0),regs.read(0x6101f4),regs.read(0x6101f8),
                regs.read(0x640000),regs.read(0x640004),regs.read(0x6100ac),regs.read(0x6101d4+head as usize*0x800));
        }
        let console=result?;
        if native::memory_requested() {
            let start=timer::rdtsc();
            inventory::probe(&mut regs,&device,&mut layout,&console)?;
            crate::serial_println!("GT710_PROBE_TIME inventory us={}",channel::elapsed(start));
            let start=timer::rdtsc();
            vm_probe::probe(&mut regs,&device,&mut layout,&console)?;
            crate::serial_println!("GT710_PROBE_TIME vm-lifetime us={}",channel::elapsed(start));
            let start=timer::rdtsc();
            aperture_probe::probe(&mut regs,&device,&mut layout,&console)?;
            crate::serial_println!("GT710_PROBE_TIME aperture us={}",channel::elapsed(start));
        }
        if native::suite_requested() {
            let start=timer::rdtsc();
            channel::probe(&mut regs,&device,&mut layout,&console)?;
            crate::serial_println!("GT710_PROBE_TIME channel us={}",channel::elapsed(start));
            let start=timer::rdtsc();
            ce::probe(&mut regs,&device,&mut layout,&console)?;
            crate::serial_println!("GT710_PROBE_TIME ce-completion us={}",channel::elapsed(start));
            let start=timer::rdtsc();
            // GR is independent of the native display. Its refused probe must
            // not discard the surface that takeover has already made active.
            if let Err(error)=gr::probe(&mut regs,&device,&mut layout,&console) {
                crate::serial_println!("GT710_GR_REFUSAL {}",error);
            }
            crate::serial_println!("GT710_PROBE_TIME graphics us={}",channel::elapsed(start));
        }
        let (_,displacement,_)=modeset::bar1_view(layout.objects[0].range.start,layout.objects[0].range.size,device.bars[1].size,17)?;
        ce::retain(layout,regs.0,console.pointer-displacement)?;
        return Ok(console);
    }
    Err("no active board-routed TMDS sink responded to live DDC")
}
pub fn identify() -> Result<Option<ConsoleSurface>, &'static str> {
    let Some(device)=pci::find_device(0x10de,0x128b) else {return Ok(None);};
    match inventory(device) {
        Ok(surface)=>Ok(Some(surface)),
        Err(e)=>{crate::serial_println!("GT710_PROBE failed: {}",e);Err(e)},
    }
}

fn display_memory(
    regs: &mut Registers,
    device: &pci::Device,
    width: u32,
    height: u32,
) -> Result<memory::Layout, &'static str> {
    let bar = device.bars[1];
    if bar.address == 0 || bar.is_io || !bar.prefetchable {
        return Err("missing display BAR1");
    }
    crate::serial_println!(
        "GT710_PROBE memory-instances display={:#x} bar1={:#x} bar2={:#x} fbp={} fbpa={}",
        regs.read(0x610010),
        regs.read(0x1704),
        regs.read(0x1714),
        regs.read(0x22438),
        regs.read(0x2243c)
    );
    // Retain the inherited DMA controls even when reservation discovery refuses
    // them, so an unsupported POST state can be resolved without GPU writes.
    for channel in 0..9 {
        crate::serial_println!(
            "GT710_PROBE memory-channel index={} control={:#x} push={:#x}",
            channel,
            regs.read(0x610490 + channel * 16),
            regs.read(0x610494 + channel * 16)
        );
    }
    let inventory = memory::discover(regs, bar.size)?;
    let layout = memory::Layout::new(inventory.total, &inventory.reserved, width, height)?;
    if !native::measuring() {
        return Ok(layout);
    }
    let before_window = regs.read(0x1700);
    let seed = (timer::rdtsc() as u32) | 1;
    let mut p = memory::Pramin::new(regs, inventory.total);
    // Small actual inherited objects are snapshotted; the allocator additionally
    // excludes their entire upper envelope and the whole low BAR aperture.
    let mut protected = inventory.references.clone();
    protected.extend([
        memory::Range {
            start: 0,
            size: 256 << 10,
        },
        memory::Range {
            start: inventory.total - (1 << 20),
            size: 1 << 20,
        },
    ]);
    protected.sort_by_key(|r| (r.start, r.size));
    protected.dedup();
    let mut snapshots = Vec::new();
    let mut protected_data = Vec::new();
    for r in &protected {
        let b = p.bytes(*r)?;
        protected_data.extend_from_slice(&b);
        snapshots.push(b);
    }
    let protected_before = hash(&protected_data);
    let protected_bytes = protected_data.len();
    let mut guards = Vec::new();
    for o in &layout.objects {
        guards.extend([
            memory::Range {
                start: o.range.start - 4096,
                size: 4096,
            },
            memory::Range {
                start: o.range.start + o.range.size,
                size: 4096,
            },
        ]);
    }
    let mut guard_data = Vec::new();
    for r in &guards {
        guard_data.extend_from_slice(&p.bytes(*r)?);
    }
    let guard_before = hash(&guard_data);
    let guard_bytes = guard_data.len();
    use crate::block::install::sha256::Sha256;
    let mut hashes: Vec<_> = layout.objects.iter()
        .map(|_| [Sha256::new(), Sha256::new()]).collect();
    crate::serial_println!(
        "GT710_PROBE memory total={} reserved-low={} owned-objects={}",
        inventory.total,
        bar.size,
        layout.objects.len()
    );
    let verified = memory::exercise(&mut p, &layout, seed, |phase, index, data| {
        hashes[index][phase].update(data);
    })?;
    protected_data.clear();
    for (r, before) in protected.iter().zip(snapshots.iter()) {
        let after = p.bytes(*r)?;
        if after != *before {
            return Err("inherited VRAM storage changed");
        }
        protected_data.extend_from_slice(&after);
    }
    guard_data.clear();
    for r in &guards {
        guard_data.extend_from_slice(&p.bytes(*r)?);
    }
    let protected_after = hash(&protected_data);
    let guard_after = hash(&guard_data);
    drop(p);
    let after_window = regs.read(0x1700);
    if before_window != after_window {
        return Err("PRAMIN control was not restored");
    }
    let mut objects = String::new();
    for (o, h) in layout.objects.iter().zip(hashes) {
        let [first, second] = h;
        let h = [hex(&first.finalize()), hex(&second.finalize())];
        if !objects.is_empty() {
            objects.push(',');
        }
        let _ = write!(
            objects,
            "{{\"name\":\"{}\",\"start\":{},\"size\":{},\"align\":{},\"sha256\":[\"{}\",\"{}\"]}}",
            o.name, o.range.start, o.range.size, o.align, h[0], h[1]
        );
    }
    let pairs = |ranges: &[memory::Range]| {
        ranges
            .iter()
            .map(|r| format!("[{},{}]", r.start, r.size))
            .collect::<Vec<_>>()
            .join(",")
    };
    let registers = inventory
        .registers
        .iter()
        .map(|(r, v)| format!("[{},{}]", r, v))
        .collect::<Vec<_>>()
        .join(",");
    let report=format!(concat!("{{\"schema\":1,\"phase\":\"memory\",\"bdf\":\"{:02x}:{:02x}.{}\",",
        "\"total\":{},\"aperture\":{},\"pitch\":{},\"width\":{},\"height\":{},\"seed\":{},",
        "\"reserved\":[{}],\"references\":[{}],\"registers\":[{}],",
        "\"pramin_before\":{},\"pramin_after\":{},\"protected_bytes\":{},\"protected_before\":\"{}\",\"protected_after\":\"{}\",",
        "\"guard_bytes\":{},\"guard_before\":\"{}\",\"guard_after\":\"{}\",\"objects\":[{}],\"verified_bytes\":{}}}"),
        device.bus,device.device,device.function,inventory.total,bar.size,layout.pitch,width,height,seed,
        pairs(&inventory.reserved),pairs(&inventory.references),registers,before_window,after_window,
        protected_bytes,protected_before,protected_after,guard_bytes,guard_before,guard_after,objects,verified);
    crate::serial_println!("GT710_MEMORY {}", report);
    Ok(layout)
}
