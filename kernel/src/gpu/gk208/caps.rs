/*
Native capability evidence derived from NVIDIA open-gpu-doc at
9fdf5c4062007929d9f4e6cbad9c9771fe61b880:
classes/{host/cla06f.h,3d/cla197.h}:
Copyright (c) 2020, NVIDIA CORPORATION. All rights reserved.
classes/dma-copy/cla0b5.h:
Copyright (c) 2019, NVIDIA CORPORATION. All rights reserved.
classes/display/cl927d.h:
Copyright (c) 1993-2022 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
Chip selection from nouveau nvkm/engine/device/base.c (Linux v6.8):
Copyright 2012 Red Hat Inc.
Class selection from nouveau nvkm/engine/fifo/gk208.c (Linux v6.8):
Copyright 2013 Red Hat Inc.
2D limit from Mesa 24.3.4 src/gallium/drivers/nouveau/nvc0/nvc0_screen.c:
Copyright 2010 Christoph Bumiller

MIT License
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
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
*/
//! Native CAPS from a snapshot of successful object creation and live display state.
//! The installation owner supplies this snapshot after binding the channel;
//! creation evidence requires more than an encoder or a source class table.
use gpu_abi::{self as abi, capability as c, capability_origin as origin, Plain};

#[derive(Clone, Copy)]
pub struct BoardState {
    pub chipset: u16,
    pub probes: ProbeResults,
    /// Bound native instance and reset epoch to which creation evidence belongs.
    pub channel: (u64, u64),
    /// Boot identity, enabled engine/runlist and graphics initialization succeeded.
    pub graphics_ready: bool,
    /// Only successful creations on the bound channels (EVO for 927D,
    /// GPFIFO/CE/GR for A06F/A0B5/A197), retained for this instance/epoch.
    pub created_classes: [u32; 4],
    /// Live native display/EDID snapshot, not firmware geometry or ARM sizes.
    pub scanout_count: u32,
    pub scanouts: [abi::ScanoutInfo; 16],
}
impl BoardState {
    pub const fn empty() -> Self {
        Self { chipset: 0, probes: ProbeResults::new(), channel: (0, 0), graphics_ready: false,
            created_classes: [0; 4], scanout_count: 0,
            scanouts: [abi::ScanoutInfo { enabled: 0, width: 0, height: 0, reserved: 0 }; 16] }
    }
}

pub struct Profile {
    caps: abi::Caps,
    have: [bool; c::LAST as usize + 1],
}
impl Profile {
    pub fn from_board(board: &BoardState, bound: (u64, u64)) -> Self {
        let mut caps = abi::Caps::ZERO;
        caps.version = 1;
        caps.size = core::mem::size_of::<abi::Caps>() as u32;
        caps.backend = abi::backend::GK208;
        caps.catalogue_version = abi::CATALOGUE_VERSION;
        caps.formats = [abi::format::B8G8R8A8, abi::format::R8G8B8A8, abi::format::R8]
            .map(|format| abi::FormatCaps { format, flags: 0 });
        let mut p = Self { caps, have: [false; c::LAST as usize + 1] };
        // nouveau device/base.c:1869-1897,3285 selects GK208B and its engines;
        // fifo/gk208.c:69 selects A06F. See provenance document.
        if board.chipset != 0x106 || bound.0 == 0 || bound.1 == 0 || board.channel != bound {
            return p;
        }
        p.caps.features = abi::feature::NATIVE_CLASSES;
        p.caps.renderer[..6].copy_from_slice(b"GK208B");
        for id in abi::gk208::CAPSET_IDS {
            if board.created_classes.contains(&id) {
                p.caps.capset_ids[p.caps.capset_count as usize] = id;
                p.caps.capset_count += 1;
            }
        }
        let graphics = p.caps.capset_ids.contains(&abi::gk208::CAPSET_ID);
        if graphics { p.caps.capset_id = abi::gk208::CAPSET_ID; }
        p.set(c::FEATURE_3D, board.graphics_ready && graphics, origin::HARDWARE_READ);
        p.set(c::CAPABILITY_SET, p.caps.capset_count == 4, origin::CLASS_TABLE);
        // cla197.h:703-707: target width 28 bits, height 17 bits;
        // :837-843: viewport clip extents 16 bits. These are encoding bounds,
        // not a claim of a 65535-pixel chip maximum. Mesa nvc0_screen.c:123-124
        // supplies the narrower 16384 chip limit already specified by the ABI.
        let width = ((1u32 << 28) - 1).min(u16::MAX as u32).min(abi::gk208::MAX_2D_SIZE);
        let height = ((1u32 << 17) - 1).min(u16::MAX as u32).min(abi::gk208::MAX_2D_SIZE);
        p.caps.max_width = width;
        p.caps.max_height = height;
        p.set(c::TARGET_SIZE, width >= 2560 && height >= 1600, origin::CHIP_LIMIT);
        p.caps.scanout_count = board.scanout_count.min(p.caps.scanouts.len() as u32);
        for (dst, src) in p.caps.scanouts.iter_mut().zip(board.scanouts.iter()).take(p.caps.scanout_count as usize) {
            *dst = abi::ScanoutInfo { enabled: u32::from(src.enabled != 0), width: src.width, height: src.height, reserved: 0 };
        }
        for id in c::BGRA_RENDER..=c::REGION_COPY {
            p.set(id, board.probes.passed(id), origin::EXECUTABLE_PROBE);
        }
        for (index, render, sample) in [(0, c::BGRA_RENDER, c::BGRA_SAMPLE),
            (1, c::RGBA_RENDER, c::RGBA_SAMPLE), (2, 0, c::R8_SAMPLE)] {
            p.caps.formats[index].flags = if render != 0 && p.have[render as usize] { abi::format_flag::RENDER } else { 0 }
                | if p.have[sample as usize] { abi::format_flag::SAMPLE } else { 0 };
        }
        for (id, bit) in [(c::INTEGER_OPS, abi::profile::INTEGER_OPS),
            (c::TEXEL_FETCH, abi::profile::TEXEL_FETCH), (c::REGION_COPY, abi::profile::REGION_COPY)] {
            if p.have[id as usize] { p.caps.profile |= bit; }
        }
        p
    }
    fn set(&mut self, id: u32, have: bool, source: u32) {
        self.have[id as usize] = have;
        if have {
            let packed = u64::from(self.caps.capability_origins[0])
                | (u64::from(self.caps.capability_origins[1]) << 32)
                | (u64::from(source) << ((id - 1) * 3));
            self.caps.capability_origins = [packed as u32, (packed >> 32) as u32];
        }
    }
    pub fn admission(&self, masked: Option<u32>) -> Result<(), (u32, u32)> {
        match (1..=c::LAST).find(|&id| !self.have[id as usize] || masked == Some(id)) {
            Some(id) => Err((abi::status::MISSING_CAPABILITY, id)),
            None => Ok(()),
        }
    }
    pub fn record(&self, masked: Option<u32>) -> abi::Caps {
        let mut caps = self.caps;
        match self.admission(masked) {
            Ok(()) => caps.status = abi::status::ADMITTED,
            Err((status, id)) => {
                caps.status = status;
                caps.missing = id;
                caps.reason[..8].copy_from_slice(b"missing ");
                let name = c::name(id).as_bytes();
                caps.reason[8..8 + name.len()].copy_from_slice(name);
            }
        }
        caps
    }
}


/// Literal, tightly packed device-output oracles. The runner consumes these
/// only for comparison. See caps/README.md for input recipes.
pub const PROBE_WIDTH: u32 = 80;
pub const PROBE_HEIGHT: u32 = 16;
pub const PROBE_BYTES: usize = PROBE_WIDTH as usize * PROBE_HEIGHT as usize * 4;
pub fn probe_expected(id: u32) -> Option<&'static [u8]> {
    match id {
        c::BGRA_RENDER | c::BGRA_SAMPLE | c::TEXEL_FETCH => Some(include_bytes!("caps/colour.bgra")),
        c::RGBA_RENDER => Some(include_bytes!("caps/colour.rgba")),
        c::RGBA_SAMPLE => Some(include_bytes!("caps/colour.bgra")),
        c::R8_SAMPLE => Some(include_bytes!("caps/r8.bgra")),
        c::INTEGER_OPS => Some(include_bytes!("caps/integer.bgra")),
        c::REGION_COPY => Some(include_bytes!("caps/region.bgra")),
        _ => None,
    }
}
#[derive(Clone, Copy)]
pub struct ProbeResults { exact: u16 }
impl ProbeResults {
    pub const fn new() -> Self { Self { exact: 0 } }
    pub fn record(&mut self, id: u32, completed: bool, first: &[u8], second: &[u8]) {
        let Some(expected) = probe_expected(id) else { return };
        let bit = 1 << id;
        self.exact &= !bit;
        if completed && first == expected && second == expected { self.exact |= bit; }
    }
    fn passed(self, id: u32) -> bool { self.exact & (1 << id) != 0 }
}
#[cfg(target_os = "none")]
mod executable;
#[cfg(target_os = "none")]
pub(in crate::gk208) use executable::run;
