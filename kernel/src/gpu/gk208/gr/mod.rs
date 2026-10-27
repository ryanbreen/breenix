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
//! Native PGRAPH startup. Context construction follows in the next unit.
pub mod ctxctl;
pub mod firmware;
pub mod tables;
pub mod topology;

/// Verify committed inputs before touching Falcon memory.
pub fn probe_static_inputs() -> Result<(), &'static str> {
    for (words, expected) in [
        (&firmware::FECS_CODE_BIN[..], firmware::FECS_CODE_BIN_SHA256),
        (&firmware::FECS_DATA_BIN[..], firmware::FECS_DATA_BIN_SHA256),
        (
            &firmware::GPCCS_CODE_BIN[..],
            firmware::GPCCS_CODE_BIN_SHA256,
        ),
        (
            &firmware::GPCCS_DATA_BIN[..],
            firmware::GPCCS_DATA_BIN_SHA256,
        ),
    ] {
        let mut hash = crate::block::install::sha256::Sha256::new();
        for word in words {
            hash.update(&word.to_le_bytes());
        }
        if super::hex(&hash.finalize()) != expected {
            return Err("GR static firmware integrity mismatch");
        }
    }
    Ok(())
}

use super::{memory, Registers};
use alloc::{format, string::String, vec::Vec};

impl ctxctl::Access for Registers {
    fn read(&mut self, address: u32) -> u32 {
        Registers::read(self, address as usize)
    }
    fn write(&mut self, address: u32, value: u32) {
        Registers::write(self, address as usize, value);
    }
    fn now_us(&mut self) -> u64 {
        super::channel::elapsed(0)
    }
    fn relax(&mut self) {
        core::hint::spin_loop();
    }
}
fn hardware_error(error: ctxctl::Fault) -> &'static str {
    crate::serial_println!(
        "GT710_GR_ERROR reason={} address={:#x} value={:#x} elapsed_us={} bound_us={}",
        error.reason,
        error.address,
        error.value,
        error.elapsed_us,
        ctxctl::BOUND_US
    );
    error.reason
}

/// Startup fragment only: no instance or golden image exists until context
/// construction. The full graphics scorer must continue to reject this fragment.
pub(super) fn probe(
    regs: &mut Registers,
    device: &crate::drivers::pci::Device,
    layout: &mut memory::Layout,
) -> Result<(), &'static str> {
    probe_static_inputs()?;
    ctxctl::reset(regs).map_err(hardware_error)?;
    let raw = [
        [0x409604, regs.read(0x409604)],
        [0x502608, regs.read(0x502608)],
        [0x500c30, regs.read(0x500c30)],
    ];
    let topology = topology::decode(&raw)?;
    let baseline = layout.allocator.stats();
    let total = baseline.reserved + baseline.charged + baseline.free;
    // Kept charged while PGRAPH can access these scratch pages. Later context
    // construction will own drain/reset and retirement. Error paths retain backing.
    let scratch_bytes = 1u64 << if regs.read(0x100c80) & 1 != 0 { 16 } else { 17 };
    let mut mmu = [0; 2];
    for (index, name) in ["gr-mmu-write", "gr-mmu-read"].into_iter().enumerate() {
        let allocation =
            layout
                .allocator
                .allocate(scratch_bytes, 4096, memory::allocator::Kind::Engine)?;
        mmu[index] = allocation.range.start;
        layout.objects.push(memory::Object {
            name,
            range: allocation.range,
            align: 4096,
        });
        let mut aperture = memory::Pramin::new(regs, total);
        for offset in (0..scratch_bytes).step_by(4) {
            aperture.write(mmu[index] + offset, 0)?;
        }
    }
    let mut withheld = ctxctl::Controller::upload(regs, &topology, mmu).map_err(hardware_error)?;
    let withheld = withheld.withheld_start(regs).map_err(hardware_error)?;
    ctxctl::reset(regs).map_err(hardware_error)?;
    let mut controller =
        ctxctl::Controller::upload(regs, &topology, mmu).map_err(hardware_error)?;
    let ready = controller.start(regs).map_err(hardware_error)?;
    // The previous real reply is the startup scratch acknowledgement. Plant
    // it and withhold method 1; no instance-backed command is issued here.
    let previous = regs.read(0x409800);
    let stale = controller
        .stale_reply(regs, previous)
        .map_err(hardware_error)?;
    let firmware: Vec<String> = [
        ("fecs.code.bin", firmware::FECS_CODE_BIN_SHA256),
        ("fecs.data.bin", firmware::FECS_DATA_BIN_SHA256),
        ("gpccs.code.bin", firmware::GPCCS_CODE_BIN_SHA256),
        ("gpccs.data.bin", firmware::GPCCS_DATA_BIN_SHA256),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (name, hash))| {
        format!(
            "\"{}\":{{\"sha256\":\"{}\",\"readback_sha256\":\"{}\"}}",
            name,
            hash,
            super::hex(&controller.readback_sha256[i])
        )
    })
    .collect();
    // Snapshot startup elapsed separately from the deliberately bounded stale
    // control, which consumes its timeout and cannot be scored as a success.
    let elapsed = ready.elapsed_us;
    let raw_counts = [[raw[0][0], raw[0][1]], [raw[1][0], raw[1][1]]];
    crate::serial_println!("GT710_GR {{\"schema\":1,\"bdf\":\"{:02x}:{:02x}.{}\",\"chipset\":{},\"firmware\":{{{}}},\"status\":{:?},\"elapsed_us\":{},\"bound_us\":{},\"fecs_size\":{},\"queries\":[],\"withheld_start\":{{\"started\":{},\"ready\":{},\"poison\":{},\"reply\":{},\"accepted\":{}}},\"stale_reply\":{{\"method\":{},\"poison\":{},\"planted\":{},\"reply\":{},\"accepted\":{}}},\"topology_raw\":{:?},\"topology\":{{\"gpc_nr\":{},\"tpc_nr\":{:?}}}}}",
        device.bus,device.device,device.function,(regs.read(0) >> 20) & 0x1ff,
        firmware.join(","),ready.status,elapsed,ctxctl::BOUND_US,ready.size,
        withheld.started,withheld.ready,withheld.poison,withheld.reply,withheld.accepted,
        stale.method,stale.poison,stale.planted,stale.reply,stale.accepted,
        raw_counts,topology.gpc_nr,&topology.tpc_nr[..usize::from(topology.gpc_nr)]);
    crate::serial_println!(
        "GT710_GR_STAGE startup controls_us={}",
        withheld.elapsed_us + stale.elapsed_us
    );
    Ok(())
}
