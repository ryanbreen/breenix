// SPDX-License-Identifier: MIT
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
                if sample.crc != 0 && sample.crc2 != 0 {
                    // Record only normalized written entries; retain the first
                    // entry, but exclude it from the stability comparison.
                    if sample.status != 1 {
                        return Err("CRC sample is not written");
                    }
                    if index >= 2 && sample != samples[1] {
                        return Err("CRC output is unstable");
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
    // The disarm completion can race another raster entry. Read the finished
    // notifier tail too, retaining those raw entries and checking their pair.
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
        if sample.status != 1 || sample.crc2 == 0 || sample != samples[1] {
            return Err("CRC completed notifier tail is unstable or partial");
        }
        samples.push(sample);
    }
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
