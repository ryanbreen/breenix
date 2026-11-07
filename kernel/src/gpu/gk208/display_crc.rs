// SPDX-License-Identifier: MIT
/*
Copyright (c) 2019, NVIDIA CORPORATION. All rights reserved.

Permission is hereby granted, free of charge, to any person obtaining a
copy of this software and associated documentation files (the "Software"),
to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included
in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR
OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.
*/
// Hardware facts: nouveau dispnv50/{crc.c,crc907d.c}, Linux v6.8 (MIT).
// The method encoder's original MIT grant is in modeset.rs and THIRD_PARTY_NOTICES.
//! Bounded native output observation for G29(3), independent of picture approval.
//! ARM's counterpart is the latched-binding readback and host-renderer attestation
//! described in docs/gui/screen-proof.md; virtual backends have no display CRC.
use super::modeset;
use alloc::vec::Vec;

pub const TIMEOUT_US: u64 = 250_000;
pub const MAX_SAMPLES: usize = 255;
const CLEANUP_US: u64 = 20_000;

pub use gpu_abi::screen_crc::{Binding, OutputConfig, Record, Sample};
#[derive(Clone, Copy)]
pub struct Checkpoint<'a> {
    pub suite: &'a str,
    pub case: &'a str,
    pub checkpoint: &'a str,
}

/// The offsets/layout are crc907d_notifier: two header words and 255 16-byte
/// entries. Entry status is undocumented; crc.c tests output_crc[0] for written.
pub fn notifier_offset(index: usize) -> Result<u64, &'static str> {
    if index >= MAX_SAMPLES {
        return Err("CRC notifier extent exhausted");
    }
    Ok(8 + index as u64 * 16)
}

pub trait Io: modeset::Io {
    fn now_us(&self) -> u64;
    fn binding(&mut self) -> Result<Binding, &'static str>;
    fn clear(&mut self) -> Result<(), &'static str>;
    /// Submit a fresh notifier-scoped core UPDATE and wait within the deadline.
    fn update(&mut self, methods: &[(u32, u32)], deadline: u64) -> Result<(), &'static str>;
    fn context_status(&mut self) -> Result<u32, &'static str>;
    fn sample(&mut self, index: usize) -> Result<Option<Sample>, &'static str>;
    fn display_error(&mut self) -> u32;
    /// Restore the pinned surface through the existing synchronous present path.
    fn recover_present(&mut self) -> Result<(), &'static str>;
}

/// The bounded core UPDATE operations shared by capture and explicit restoration.
pub trait OutputIo: modeset::Io {
    fn output_now_us(&self) -> u64;
    fn output_update(&mut self, methods: &[(u32, u32)], deadline: u64)
        -> Result<(), &'static str>;
}
impl<T: Io> OutputIo for T {
    fn output_now_us(&self) -> u64 { self.now_us() }
    fn output_update(&mut self, methods: &[(u32, u32)], deadline: u64)
        -> Result<(), &'static str> { self.update(methods, deadline) }
}

/// Saved active output state, retained until a verified restoring UPDATE.
/// The native session serializes checkpoints, disarm and its other core methods.
#[derive(Default)]
pub struct Perturbation {
    saved: Option<(u32, u32, Vec<(u32, u32)>)>,
}
impl Perturbation {
    pub const fn new() -> Self { Self { saved: None } }
    pub fn active(&self) -> bool { self.saved.is_some() }
    pub fn enable(&mut self, io: &mut impl OutputIo, head: u32, sor: u32)
        -> Result<(), &'static str> {
        if self.active() { return Err("output perturbation awaits disarm"); }
        let before = modeset::crc_output_methods(io, head, sor)?;
        let previous = before.iter().find(|p| p.0 == 0x4a0 + head * 0x300)
            .ok_or("output dithering method absent")?.1;
        let methods = modeset::output_dither_methods(head, previous, true)?;
        // Save before publication: even a partially accepted UPDATE needs disarm.
        self.saved = Some((head, sor, before));
        io.output_update(&methods, io.output_now_us().saturating_add(CLEANUP_US))?;
        let actual = modeset::crc_output_methods(io, head, sor)?;
        let expected = &self.saved.as_ref().unwrap().2;
        // Only the requested dithering value may differ from the active list.
        if actual.len() != expected.len() || actual.iter().zip(expected).any(|(a, b)|
            a.0 != b.0 || a.1 != if a.0 == methods[0].0 { methods[0].1 } else { b.1 }) {
            return Err("output perturbation did not latch");
        }
        Ok(())
    }
    pub fn restore(&mut self, io: &mut impl OutputIo) -> Result<(), &'static str> {
        let Some((head, sor, before)) = self.saved.as_ref() else { return Ok(()); };
        let previous = before.iter().find(|p| p.0 == 0x4a0 + head * 0x300)
            .ok_or("saved output dithering method absent")?.1;
        let methods = modeset::output_dither_methods(*head, previous, false)?;
        io.output_update(&methods, io.output_now_us().saturating_add(CLEANUP_US))?;
        if modeset::crc_output_methods(io, *head, *sor)? != *before {
            return Err("output perturbation restoration differs");
        }
        self.saved = None;
        Ok(())
    }
}

/// Consume the one-shot only at a matching, pinned native checkpoint, after the
/// enabling method has latched. A failed publication remains restorable.
#[cfg(target_os = "none")]
pub fn perturb_checkpoint(io: &mut impl Io, state: &mut Perturbation,
    expected: Binding, head: u32, sor: u32) -> Result<(), &'static str> {
    if let Some(arm) = crate::test_control::reserve(
        breengel_abi::tc_point::DISPLAY_OUTPUT_PERTURB, expected.resource) {
        if expected.resource == 0 || expected.present_fence == 0 || io.binding()? != expected {
            return Err("output perturbation binding differs");
        }
        state.enable(io, head, sor)?;
        arm.fire();
    }
    Ok(())
}

/// DISARM_ALL's driver boundary; ARM has no equivalent display output CRC.
#[cfg(target_os = "none")]
pub fn restore_perturbation() -> Result<(), &'static str> {
    super::ce::restore_output_perturbation()
}

fn output(
    io: &mut impl Io,
    mode: [u32; 2],
    head: u32,
    sor: u32,
) -> Result<OutputConfig, &'static str> {
    let methods = modeset::crc_output_methods(io, head, sor)?;
    let value = |offset| {
        methods
            .iter()
            .find(|p| p.0 == offset + head * 0x300)
            .unwrap()
            .1
    };
    let blank = value(0x41c);
    let end = value(0x420);
    let raster = [
        (end & 0xffff)
            .checked_sub(blank & 0xffff)
            .ok_or("CRC raster width")?,
        (end >> 16)
            .checked_sub(blank >> 16)
            .ok_or("CRC raster height")?,
    ];
    let out = value(0x4c0);
    let (w, h) = (out & 0x7fff, (out >> 16) & 0x7fff);
    let input = value(0x4b8);
    let route = methods
        .iter()
        .find(|p| p.0 == 0x200 + sor * 0x20)
        .unwrap()
        .1;
    if mode.contains(&0)
        || mode[0] > input & 0x7fff
        || mode[1] > (input >> 16) & 0x7fff
        || w == 0
        || h == 0
        || w > raster[0]
        || h > raster[1]
        || value(0x4bc) != 0
        || value(0x404) & 3 != 0
        || route & 15 != 1 << head
        || !matches!((route >> 8) & 15, 1 | 2)
    {
        return Err("unsupported CRC raster, viewport or output route");
    }
    Ok(OutputConfig {
        mode,
        raster,
        viewport: [(raster[0] - w) / 2, (raster[1] - h) / 2, w, h],
        sor,
        rg: head,
        methods,
    })
}

/// Caller holds the shared observation transition across pixels and this CRC.
/// The I/O implementation independently verifies the physical binding as well.
#[allow(clippy::too_many_arguments)]
pub fn capture(
    io: &mut impl Io,
    checkpoint: Checkpoint<'_>,
    expected: Binding,
    mode: [u32; 2],
    head: u32,
    sor: u32,
    handle: u32,
) -> Result<Record, &'static str> {
    let arm = modeset::crc_methods(head, sor, handle, true)?;
    let disarm = modeset::crc_methods(head, sor, handle, false)?;
    let start = io.now_us();
    let deadline = start.saturating_add(TIMEOUT_US);
    let mut arm_failed = false;
    let result = (|| {
        if expected.resource == 0 || expected.present_fence == 0 || io.binding()? != expected {
            return Err("CRC binding differs before arm");
        }
        let config = output(io, mode, head, sor)?;
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(MAX_SAMPLES)
            .map_err(|_| "CRC samples allocation")?;
        io.clear()?;
        if let Err(error) = io.update(&arm, deadline) {
            arm_failed = true;
            return Err(error);
        }
        loop {
            if io.now_us() >= deadline.saturating_sub(CLEANUP_US) {
                return Err("CRC sample deadline");
            }
            if io.binding()? != expected {
                return Err("CRC binding changed");
            }
            if io.display_error() != 0 {
                return Err("CRC display error");
            }
            if io.context_status()? & 0x3e != 0 {
                return Err("CRC notifier overflow");
            }
            let index = samples.len();
            if index == MAX_SAMPLES {
                return Err("CRC notifier extent exhausted");
            }
            if let Some(sample) = io.sample(index)? {
                if sample.crc != 0 {
                    // Primary writes bound collection. Secondary words are
                    // interpreted after disarm and DONE, per NVIDIA's MIT
                    // Display-CRC/display-crc.txt capture sequence and layout.
                    if sample.status != 1 {
                        return Err("CRC sample is not written");
                    }
                    samples.push(sample);
                    if samples.len() >= 5 {
                        break;
                    }
                    continue;
                }
            }
            io.delay_us(100);
        }
        if modeset::crc_output_methods(io, head, sor)? != config.methods {
            return Err("CRC output configuration changed");
        }
        Ok((config, samples))
    })();
    // Cleanup is attempted even if preflight, allocation, clear or arm failed.
    // An expired observation still gets a bounded cleanup, but cannot be accepted.
    let cleanup_deadline = deadline.max(io.now_us().saturating_add(CLEANUP_US));
    let cleanup = io.update(&disarm, cleanup_deadline);
    if arm_failed || cleanup.is_err() {
        io.recover_present()?;
    }
    cleanup?;
    let (config, mut samples) = result?;
    let status = loop {
        let status = io.context_status()?;
        if status & 0x3e != 0 {
            return Err("CRC notifier overflow after disarm");
        }
        if status & 1 != 0 {
            break status;
        }
        if io.now_us() >= deadline {
            return Err("CRC context completion deadline");
        }
        io.delay_us(100);
    };
    // DONE publishes the complete notifier, including secondary writes and
    // entries produced during disarm. Re-read its entire written extent;
    // provisional primary observations cannot establish a final CRC pair.
    samples.clear();
    while samples.len() < MAX_SAMPLES {
        if io.now_us() >= deadline {
            return Err("CRC final sample deadline");
        }
        let Some(sample) = io.sample(samples.len())? else {
            break;
        };
        if sample.crc == 0 {
            break;
        }
        if sample.status != 1 || sample.crc2 == 0
            || (samples.len() >= 2 && sample != samples[1]) {
            return Err("CRC completed notifier tail is unstable or partial");
        }
        samples.push(sample);
    }
    if samples.len() < 5 { return Err("CRC completed notifier lacks quorum"); }
    let binding_after = io.binding()?;
    if binding_after != expected {
        return Err("CRC binding changed after disarm");
    }
    let display_error = io.display_error();
    if modeset::crc_output_methods(io, head, sor)? != config.methods {
        return Err("CRC output configuration changed after disarm");
    }
    let elapsed = io.now_us().saturating_sub(start);
    if display_error != 0 || elapsed > TIMEOUT_US {
        return Err("CRC output error or deadline");
    }
    Ok(Record {
        suite: checkpoint.suite.into(),
        case: checkpoint.case.into(),
        checkpoint: checkpoint.checkpoint.into(),
        output_config: config,
        binding_before: expected,
        binding_after,
        elapsed_us: elapsed as u32,
        disarmed: true,
        display_error,
        status,
        samples,
    })
}

/// Read a bounded snapshot after an observation fails and attempts cleanup.
/// The success path does not request this diagnostic snapshot.
pub fn failure_snapshot(io: &mut impl Io, error: &str, head: u32, sor: u32) -> alloc::string::String {
    use core::fmt::Write;
    let mut text = alloc::format!("error={} head={head} sor={sor}", error.replace(' ', "_"));
    for register in [0x61009c, 0x6100ac, 0x616308 + head as usize * 0x800, 0x640004] {
        let value = io.read(register);
        let _ = write!(text, " r{register:06x}={value:08x}");
    }
    match io.context_status() {
        Ok(status) => { let _ = write!(text, " status={status:08x}"); }
        Err(error) => { let _ = write!(text, " status_error={}", error.replace(' ', "_")); }
    }
    // Keep the viewport, output route and dithering bank pairs.
    for method in [0x200 + sor * 0x20, 0x404 + head * 0x300,
        0x41c + head * 0x300, 0x420 + head * 0x300, 0x4a0 + head * 0x300,
        0x4b8 + head * 0x300, 0x4bc + head * 0x300, 0x4c0 + head * 0x300] {
        let active = io.read(0x640000 + method as usize);
        let assembly = io.read(0x660000 + method as usize);
        let _ = write!(text, " method={method:08x} active={active:08x} assembly={assembly:08x}");
    }
    for index in 0..5 {
        match io.sample(index) {
            Ok(Some(sample)) => { let _ = write!(text, " sample{index}={:08x}:{:08x}", sample.crc, sample.crc2); }
            Ok(None) => { let _ = write!(text, " sample{index}=unwritten"); }
            Err(error) => { let _ = write!(text, " sample{index}={}", error.replace(' ', "_")); }
        }
    }
    text
}

/// Failure-only diagnostic: ask the documented primary selector for RG data.
/// This returns text, not an eligible checkpoint or a substitute CRC pair.
pub fn probe_primary_rg(io: &mut impl Io, expected: Binding, head: u32, sor: u32,
    handle: u32, secondary_sor: bool) -> alloc::string::String {
    use core::fmt::Write;
    let result = (|| -> Result<alloc::string::String, &'static str> {
        let mut arm = modeset::crc_methods(head, sor, handle, true)?;
        // NVIDIA Display-CRC: primary RG(i)=0xff8+i; secondary can name SOR.
        let secondary = if secondary_sor { 0xf0f + sor * 16 } else { 0xfff };
        arm[1].1 = (secondary << 20) | ((0xff8 + head) << 8) | 0x40;
        let disarm = modeset::crc_methods(head, sor, handle, false)?;
        let deadline = io.now_us().saturating_add(150_000);
        let mut armed = false;
        let acquired = (|| -> Result<(), &'static str> {
            if io.binding()? != expected { return Err("RG probe binding changed"); }
            io.clear()?;
            armed = true;
            io.update(&arm, deadline)?;
            let mut count = 0;
            while count < 5 {
                if io.now_us() >= deadline.saturating_sub(CLEANUP_US) {
                    return Err("RG probe primary deadline");
                }
                if io.binding()? != expected || io.display_error() != 0 {
                    return Err("RG probe binding or display error");
                }
                if io.context_status()? & 0x3e != 0 { return Err("RG probe overflow"); }
                if io.sample(count)?.is_some_and(|s| s.crc != 0) { count += 1; }
                else { io.delay_us(100); }
            }
            Ok(())
        })();
        let cleanup = io.update(&disarm, deadline.max(io.now_us().saturating_add(CLEANUP_US)));
        if (armed && acquired.is_err()) || cleanup.is_err() { io.recover_present()?; }
        cleanup?;
        acquired?;
        let status = loop {
            let status = io.context_status()?;
            if status & 0x3e != 0 { return Err("RG probe completed overflow"); }
            if status & 1 != 0 { break status; }
            if io.now_us() >= deadline { return Err("RG probe completion deadline"); }
            io.delay_us(100);
        };
        if io.binding()? != expected { return Err("RG probe final binding changed"); }
        let mut text = alloc::format!("rg_secondary_sor={} rg_status={status:08x}", u8::from(secondary_sor));
        for i in 0..5 {
            let sample = io.sample(i)?.ok_or("RG probe completed entry absent")?;
            if i == 0 { let _ = write!(text, " rg_primary={:08x}", sample.crc); }
            let _ = write!(text, " rg_sample{i}={:08x}:{:08x}", sample.crc, sample.crc2);
        }
        Ok(text)
    })();
    result.unwrap_or_else(|error| alloc::format!("rg_probe_error={}", error.replace(' ', "_")))
}

/// Driver boundary for a suite checkpoint whose binding is pinned by the shared
/// display observation transition. It must remain held through this call.
/// The physical head is checked against that resource during collection; the
/// transition excludes re-presentation (and hence present-fence changes).
#[cfg(target_os = "none")]
pub fn observe(
    checkpoint: Checkpoint<'_>,
    binding: Binding,
    mode: [u32; 2],
) -> Result<Record, &'static str> {
    if !crate::test_control::enabled() {
        return Err("CRC observation requires a suite boot");
    }
    super::ce::capture_crc(checkpoint, binding, mode)
}
