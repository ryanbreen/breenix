/*
 * Copyright 2010 Christoph Bumiller
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
 * THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 */
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
 * rnndb copyright 2006
 * Artur Huillet
 * Ben Skeggs
 * B. R.
 * Carlos Martin
 * Christoph Bumiller
 * Dawid Gajownik
 * Dmitry Baryshkov
 * Dmitry Eremin-Solenikov
 * EdB
 * Erik Waling
 * Francisco Jerez
 * Ilia Mirkin
 * jb17bsome
 * Jeremy Kolb
 * Laurent Carlier
 * Luca Barbieri
 * Maarten Maathuis
 * Marcelina Kościelnicka
 * Mark Carey
 * Matthieu Castet
 * nvidiaman
 * Patrice Mandin
 * Pekka Paalanen
 * Peter Popov
 * Richard Hughes
 * Rudi Cilibrasi
 * Serge Martin
 * Simon Raffeiner
 * Stephane Loeuillet
 * Stephane Marchesin
 * sturmflut
 * Sylvain Munaut
 * Victor Stinner
 * Wladmir van der Laan
 * Younes Manton
 *
 * Permission is hereby granted, free of charge, to any person obtaining
 * a copy of this software and associated documentation files (the
 * "Software"), to deal in the Software without restriction, including
 * without limitation the rights to use, copy, modify, merge, publish,
 * distribute, sublicense, and/or sell copies of the Software, and to
 * permit persons to whom the Software is furnished to do so, subject to
 * the following conditions:
 *
 * The above copyright notice and this permission notice (including the
 * next paragraph) shall be included in all copies or substantial
 * portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
 * EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
 * MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
 * IN NO EVENT SHALL THE COPYRIGHT OWNER(S) AND/OR ITS SUPPLIERS BE
 * LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
 * OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
 * WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */
//! Native PGRAPH startup and golden context construction.
pub mod binding;
pub mod channel;
pub mod context;
pub mod ctxctl;
pub mod draw;
pub mod firmware;
pub mod shaders;
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

impl binding::TrapIo for Registers {
    fn read(&mut self, address: usize) -> u32 { Registers::read(self, address) }
    fn write(&mut self, address: usize, value: u32) { Registers::write(self, address, value); }
}

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
    fn restored_irqs(&mut self, expected: [u32; 5], actual: [u32; 5]) {
        crate::serial_println!(
            "GT710_GR_IRQ_RESTORED {{\"registers\":[320,8512,4194620,4234276,4231196],\"expected\":{:?},\"actual\":{:?},\"gr_accessible_readback\":true}}",
            expected, actual
        );
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

/// Suite-thread snapshot, also emitted on refused probes.
pub(super) fn interrupt_snapshot(
    regs: &mut Registers,
    device: &crate::drivers::pci::Device,
    phase: &str,
) {
    use crate::drivers::pci;
    // MIT rnndb bus/pmc.xml, fifo/gf100_pfifo.xml and gr/ctxctl.xml.
    let mut words: Vec<[u32; 2]> = [
        0x100, 0x140, 0x160, 0x200, 0x204, 0x2100, 0x2140, 0x2274,
        0x2284, 0x259c, 0x2630, 0x2634, 0x2640, 0x2644, 0x800000, 0x800004, 0x400100, 0x400108, 0x40013c,
        0x409008, 0x409018, 0x40901c, 0x409b00, 0x409b04, 0x409c18, 0x409c24,
    ]
    .into_iter()
    .map(|a| [a, regs.read(a as usize)])
    .collect();
    // MIT FIFO gf100.c:696-714: read fault records without acknowledging
    // them, before a failed preempt or outer power-state restore obscures GR.
    let faults = regs.read(0x259c);
    for unit in 0..32 {
        if faults & (1 << unit) != 0 {
            for offset in [0, 4, 8, 12] {
                let address = 0x2800 + unit * 16 + offset;
                words.push([address, regs.read(address as usize)]);
            }
        }
    }
    let enabled = words.iter().find(|w| w[0] == 0x204).unwrap()[1];
    for i in 0..3 {
        if enabled & (1 << i) != 0 {
            for offset in [0x00, 0x14, 0x28, 0x108, 0x10c, 0x120, 0x13c, 0x148, 0x14c] {
                let address = 0x40000 + i * 0x2000 + offset;
                words.push([address, regs.read(address as usize)]);
            }
        }
    }
    let xhci = pci::find_by_class(pci::DeviceClass::SerialBus, 3, 0x30);
    let routes:Vec<String> = core::iter::once(device).chain(xhci.as_ref()).map(|d| {
        let read=|offset|pci::pci_read_config_word(d.bus,d.device,d.function,offset);
        let msi=d.find_msi_capability().map(|c|format!("{}",read(c+2))).unwrap_or_else(||"null".into());
        let msix=d.find_msix_capability().map(|c|format!("{}",read(c+2))).unwrap_or_else(||"null".into());
        format!("{{\"bdf\":\"{:02x}:{:02x}.{}\",\"line_pin\":{},\"command\":{},\"msi\":{},\"msix\":{}}}",
            d.bus,d.device,d.function,read(0x3c),read(4),msi,msix)
    }).collect();
    crate::serial_println!(
        "GT710_GR_IRQ {{\"phase\":\"{}\",\"registers\":{:?},\"routes\":[{}]}}",
        phase,
        words,
        routes.join(",")
    );
}

struct ProbeCleanup {
    state: ctxctl::SavedState,
    scratch: Vec<memory::allocator::Allocation>,
    finished: bool,
    fifo_quiesced: bool,
}
impl ProbeCleanup {
    fn finish(
        &mut self,
        regs: &mut Registers,
        layout: &mut memory::Layout,
    ) -> Result<(), &'static str> {
        if self.finished {
            return Ok(());
        }
        if !self.fifo_quiesced {
            return Err("GR FIFO unwind incomplete; backing retained");
        }
        self.state.stop(regs).map_err(hardware_error)?;
        let release = (|| {
            while let Some(&allocation) = self.scratch.last() {
                layout.allocator.release(allocation)?;
                self.scratch.pop();
            }
            Ok(())
        })();
        let restore = self.state.restore(regs).map_err(hardware_error);
        release?;
        restore?;
        self.finished = true;
        Ok(())
    }
}

/// Capture startup, golden context and one A197 channel for the raw scorer.
pub(super) fn probe(
    regs: &mut Registers,
    device: &crate::drivers::pci::Device,
    layout: &mut memory::Layout,
    console: &super::ConsoleSurface,
    evo: &mut super::native::Evo,
) -> Result<(), &'static str> {
    probe_static_inputs()?;
    let mut cleanup = ProbeCleanup {
        state: ctxctl::SavedState::capture(regs).map_err(hardware_error)?,
        scratch: Vec::new(),
        finished: false,
        fifo_quiesced: true,
    };
    let result = probe_inner(regs, device, layout, console, evo, &mut cleanup);
    // This also runs after topology, allocation, upload and startup refusals.
    let unwind = cleanup.finish(regs, layout);
    result.and(unwind)
}

fn probe_inner(
    regs: &mut Registers,
    device: &crate::drivers::pci::Device,
    layout: &mut memory::Layout,
    console: &super::ConsoleSurface,
    evo: &mut super::native::Evo,
    cleanup: &mut ProbeCleanup,
) -> Result<(), &'static str> {
    ctxctl::reset(regs).map_err(hardware_error)?;
    cleanup.state.capture_gr(regs).map_err(hardware_error)?;
    let raw = [
        [0x409604, regs.read(0x409604)],
        [0x502608, regs.read(0x502608)],
        [0x500c30, regs.read(0x500c30)],
    ];
    // Retain the source words even if decoding refuses the topology, before
    // any firmware upload. No context or image bytes travel over serial.
    crate::serial_println!("GT710_GR_TOPOLOGY {:?}", raw);
    let topology = topology::decode(&raw)?;
    let baseline = layout.allocator.stats();
    let total = baseline.reserved + baseline.charged + baseline.free;
    // Scratch backing stays charged until the confirmed GR reset below.
    let scratch_bytes = 1u64 << if regs.read(0x100c80) & 1 != 0 { 16 } else { 17 };
    let mut mmu = [0; 2];
    for index in 0..2 {
        let allocation =
            layout
                .allocator
                .allocate(scratch_bytes, 4096, memory::allocator::Kind::Engine)?;
        mmu[index] = allocation.range.start;
        cleanup.scratch.push(allocation);
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
    let context = super::lifetime::Context::new(
        &mut layout.allocator,
        &mut super::channel::VmIo { regs, total },
    )?;
    let mut hardware = Hardware {
        regs,
        layout,
        device,
        console,
        total,
        context,
        controller,
        backing: Vec::new(),
        next_va: 0x10000000,
        admitted: false,
        reader: None,
        graphics: None,
        captures: Vec::new(),
        runnable_header: Vec::new(),
    };
    // From here, only Hardware::finish can certify that reset is safe.
    cleanup.fifo_quiesced = false;
    let mut outcome = (|| {
        let shader_buffer = context::Io::allocate(&mut hardware, context::Spec {
            name: "shader-segment", bytes: shaders::SEGMENT_SIZE as u64, align: 4096,
        }).map_err(|e| e.reason)?;
        let generated = context::generate(&mut hardware, &topology, ready.size);
        if let Err(error) = &generated {
            context_error(error);
        }
        let graphics = if let Ok(golden) = &generated {
            // CE read-back owns channel 0 and its USERD alias until drained.
            if let Some(reader) = hardware.reader.as_mut() {
                reader.quiesce(hardware.regs, hardware.layout)?;
            }
            if let Some(reader) = hardware.reader.take() {
                reader.finish(hardware.regs, hardware.layout)?;
            }
            hardware.reader = Some(super::ce::readback::Reader::new(
                hardware.regs, hardware.device, hardware.layout, hardware.console, &hardware.backing,
            )?);
            let mut evidence = ShaderEvidence::default();
            shaders::exercise_uploads(&mut ShaderUpload {
                hardware: &mut hardware, buffer: shader_buffer, evidence: &mut evidence,
            })?;
            // CE owns channel 0 and the USERD alias. Drain it before restoring GR.
            if let Some(reader) = hardware.reader.as_mut() {
                reader.quiesce(hardware.regs, hardware.layout)?;
            }
            if let Some(reader) = hardware.reader.take() {
                reader.finish(hardware.regs, hardware.layout)?;
            }
            // Channel IDs remain owned by GR until reset, even after FIFO
            // quiesce: FECS retains its current context. Finish CE before GR
            // admission and keep GR active through the binding submission.
            let mut graphics = channel::Session::new(&mut hardware, golden, shader_buffer.va)?;
            let result = graphics.open_second(&mut hardware, golden, shader_buffer.va)
                .and_then(|()| graphics.activate(&mut hardware))
                .and_then(|()| graphics.execute(&mut hardware));
            // Retain ownership before propagating activation or execution errors.
            hardware.graphics = Some(graphics);
            result?;
            let mut graphics = hardware.graphics.take().ok_or("shader GR channel absent")?;
            let mut receipt = binding::Receipt {
                target: 0, epoch: 0, nonce: 0, before: [0; 4], after: [0; 4], observation: None,
            };
            let result = graphics.bind_shaders(&mut hardware, shader_buffer.va, &mut receipt);
            // Keep ownership on trap/timeout so finish drains and resets before freeing.
            hardware.graphics = Some(graphics);
            evidence.emit(shader_buffer.va, &receipt, result.as_ref().err().copied())?;
            result?;
            let mut graphics = hardware.graphics.take().ok_or("draw GR channel absent")?;
            let mut draw_evidence = DrawEvidence::default();
            let draw_result = draw_probe(&mut hardware, &mut graphics, shader_buffer.va, &mut draw_evidence);
            let draw_result = draw_result.and_then(|()|
                present_probe(&mut hardware, &mut graphics, evo, &mut draw_evidence));
            draw_evidence.trap_capture = binding::capture_trap(hardware.regs);
            if let Some(capture) = &draw_evidence.trap_capture {
                binding::acknowledge_trap(hardware.regs, capture);
            }
            let draw_result = if draw_evidence.trap_capture.is_some() {
                draw_result.and(Err("draw PGRAPH trap/exception"))
            } else { draw_result };
            hardware.graphics = Some(graphics);
            let draw_emit = draw_evidence.emit(draw_result.as_ref().err().copied());
            draw_result?;
            draw_emit?;
            // Emit the existing draw block before adding context rows: its
            // unchanged guard reserves space for the pending golden saves.
            let mut graphics = hardware.graphics.take().ok_or("context GR channel absent")?;
            let context_result = contexts_probe(&mut hardware, &mut graphics, shader_buffer.va);
            hardware.graphics = Some(graphics);
            context_result?;
            // Only retire the FIFO after the final draw and its CE snapshots.
            hardware.graphics.as_mut().ok_or("draw GR channel absent")?
                .quiesce(hardware.regs, hardware.total)?;
            let graphics = hardware.graphics.as_ref().ok_or("draw GR channel absent")?;
            graphics.preempt_result()?;
            let report = graphics.report();
            interrupt_snapshot(hardware.regs, hardware.device, "channel-drained");
            Some(report)
        } else {
            None
        };
        // Preserve raw saves even when the byte check fails. No serial image dump.
        let artifact = hardware.emit_images();
        let runnable_bytes = hardware.runnable_header.clone();
        let firmware_hashes = hardware.controller.readback_sha256;
        Ok::<_, &'static str>((generated, graphics, artifact, runnable_bytes, firmware_hashes))
    })();
    // Earlier shader/context refusals also need a first capture before reset.
    if let Some(capture) = binding::capture_trap(hardware.regs) {
        binding::acknowledge_trap(hardware.regs, &capture);
        let evidence = DrawEvidence { trap_capture: Some(capture), ..DrawEvidence::default() };
        let error = outcome.as_ref().err().copied().unwrap_or("GR PGRAPH trap/exception");
        let _ = evidence.emit(Some(error));
        outcome = Err(error);
    }
    if outcome.is_err() && hardware.regs.read(0x259c) != 0 {
        // Retain the raw MMU unit words before the owned-fault reset/W1C.
        interrupt_snapshot(hardware.regs, hardware.device, "before-unwind");
    }
    // Run on closure errors too; the outer reset requires confirmed FIFO unwind.
    if let Err(error) = hardware.finish() {
        // Keep the failing FIFO words while GR is still accessible. The outer
        // cleanup must refuse reset here, but must not replace this cause.
        interrupt_snapshot(regs, device, "unwind-failed");
        return Err(error);
    }
    cleanup.fifo_quiesced = true;
    cleanup.finish(regs, layout)?;
    let (generated, graphics, artifact, runnable_bytes, firmware_hashes) = outcome?;
    if layout.allocator.stats() != baseline {
        return Err("GR baseline reclamation mismatch");
    }
    let golden = generated.map_err(|error| error.reason)?;
    let graphics = graphics.ok_or("GR channel record missing")?;
    let (image_bulk_bytes, artifact_estimate) = artifact?;
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
            super::hex(&firmware_hashes[i])
        )
    })
    .collect();
    // Snapshot startup elapsed separately from the deliberately bounded stale
    // control, which consumes its timeout and cannot be scored as a success.
    let elapsed = ready.elapsed_us;
    let queries: Vec<String> = golden.queries.iter().map(|q| format!(
        "{{\"method\":{},\"poison\":{},\"reply\":{},\"size\":{},\"elapsed_us\":{},\"bound_us\":{}}}",
        q.method,q.poison,q.reply,q.size,q.elapsed_us,ctxctl::BOUND_US)).collect();
    let lists: Vec<String> = tables::INIT_LISTS
        .iter()
        .map(|l| {
            format!(
                "{{\"name\":\"{}\",\"count\":{},\"sha256\":\"{}\"}}",
                l.name, l.count, l.sha256
            )
        })
        .collect();
    for receipt in &golden.lists {
        let expected = &tables::INIT_LISTS[receipt.index];
        if receipt.count != expected.count
            || super::hex(&receipt.sha256) != expected.sha256
            || receipt.packs.len() != expected.packs.len()
            || receipt
                .packs
                .iter()
                .zip(expected.packs)
                .any(|(a, b)| a.count != b.init.len() || a.sha256 == [0; 32])
        {
            return Err("GR context replay digest differs");
        }
    }
    let buffers: Vec<String> = golden.buffers[..4]
        .iter()
        .map(|b| {
            format!(
                "{{\"name\":\"{}\",\"va\":{},\"bytes\":{},\"align\":{}}}",
                b.spec.name, b.va, b.spec.bytes, b.spec.align
            )
        })
        .collect();
    let saves: Vec<String> = [85, 170]
        .into_iter()
        .enumerate()
        .map(|(n, p)| {
            format!(
                "{{\"poison\":{},\"va\":{},\"sha256\":\"{}\",\"image_sha256\":\"{}\"}}",
                p,
                golden.buffers[4 + n].va,
                super::hex(&golden.hashes[n]),
                super::hash(&golden.images[n])
            )
        })
        .collect();
    let layout_words = &golden.layout;
    let units = layout_units(layout_words);
    let context_fields = format!("\"schema\":1,\"bdf\":\"{:02x}:{:02x}.{}\",\"chipset\":{},\"firmware\":{{{}}},\"status\":{:?},\"elapsed_us\":{},\"bound_us\":{},\"fecs_size\":{},\"queries\":[{}],\"withheld_start\":{{\"started\":{},\"ready\":{},\"poison\":{},\"reply\":{},\"accepted\":{}}},\"stale_reply\":{{\"method\":{},\"poison\":{},\"planted\":{},\"reply\":{},\"accepted\":{}}},\"topology_raw\":{:?},\"topology\":{{\"gpc_nr\":{},\"tpc_nr\":{:?},\"tpc_mask\":{:?}}},\"init_lists\":[{}],\"global_buffers\":[{}],\"golden_bytes\":{},\"saved_end\":{},\"save_layout\":{{\"gpc\":{},\"strands\":{},\"strand_base\":{},\"strand_words\":{},\"units\":{}}},\"saves\":[{}],\"runnable_header\":{{\"count\":{},\"va\":{},\"words\":{:?},\"sha256\":\"{}\"}},\"image_bulk_bytes\":{},\"bulk_artifact_estimate\":{},\"baseline\":{},\"after\":{},\"firmware_writes\":{},\"draw\":\"pending-7.4\",\"shaders\":\"pending-7.3\",\"context_switch\":\"pending-7.5\"",
        device.bus,device.device,device.function,(regs.read(0) >> 20) & 0x1ff,
        firmware.join(","),ready.status,elapsed,ctxctl::BOUND_US,ready.size,queries.join(","),
        withheld.started,withheld.ready,withheld.poison,withheld.reply,withheld.accepted,
        stale.method,stale.poison,stale.planted,stale.reply,stale.accepted,
        raw,topology.gpc_nr,&topology.tpc_nr[..1],&topology.tpc_mask[..1],lists.join(","),buffers.join(","),
        ready.size,golden.saved_end,layout_words.gpc,layout_words.strands,layout_words.strand_base,layout_words.strand_words,units,
        saves.join(","),golden.patches.len(),golden.buffers[3].va,golden.header,super::hash(&runnable_bytes),image_bulk_bytes,artifact_estimate,
        super::inventory::stats(baseline),super::inventory::stats(layout.allocator.stats()),
        crate::firmware_writes::count()-console.firmware_baseline);
    crate::serial_println!("{}", channel::format_record(&context_fields, &graphics));
    crate::serial_println!(
        "GT710_GR_STAGE startup controls_us={}",
        withheld.elapsed_us + stale.elapsed_us
    );
    Ok(())
}

fn context_error(error: &context::Error) -> &'static str {
    crate::serial_println!(
        "GT710_GR_ERROR reason={} offset={} first={:#04x} second={:#04x}",
        error.reason,
        error.offset,
        error.first,
        error.second
    );
    error.reason
}
struct Hardware<'a> {
    regs: &'a mut Registers,
    layout: &'a mut memory::Layout,
    device: &'a crate::drivers::pci::Device,
    console: &'a super::ConsoleSurface,
    total: u64,
    context: super::lifetime::Context,
    controller: ctxctl::Controller,
    backing: Vec<(u64, memory::allocator::Allocation)>,
    next_va: u64,
    admitted: bool,
    reader: Option<super::ce::readback::Reader>,
    graphics: Option<channel::Session>,
    captures: Vec<(u8, Vec<u8>)>,
    runnable_header: Vec<u8>,
}
impl ctxctl::Access for Hardware<'_> {
    fn read(&mut self, a: u32) -> u32 {
        Registers::read(self.regs, a as usize)
    }
    fn write(&mut self, a: u32, v: u32) {
        Registers::write(self.regs, a as usize, v);
    }
    fn now_us(&mut self) -> u64 {
        super::channel::elapsed(0)
    }
    fn relax(&mut self) {
        core::hint::spin_loop();
    }
}
impl Hardware<'_> {
    fn allocation(
        &self,
        b: context::Buffer,
    ) -> Result<memory::allocator::Allocation, context::Error> {
        self.backing
            .iter()
            .find(|(va, _)| *va == b.va)
            .map(|(_, a)| *a)
            .ok_or_else(|| "GR buffer absent".into())
    }
    fn fill(&mut self, b: context::Buffer, data: &[u8]) -> Result<(), context::Error> {
        if data.len() != b.spec.bytes as usize || data.len() % 4 != 0 {
            return Err("GR buffer write extent".into());
        }
        let a = self.allocation(b)?;
        let mut p = memory::Pramin::new(self.regs, self.total);
        for (i, w) in data.chunks_exact(4).enumerate() {
            p.write(
                a.range.start + i as u64 * 4,
                u32::from_le_bytes(w.try_into().unwrap()),
            )?;
        }
        drop(p);
        super::modeset::bar_flush(self.regs)?;
        Ok(())
    }
    fn emit_images(&mut self) -> Result<(usize, usize), &'static str> {
        let mut rows = String::new();
        use core::fmt::Write;
        for (poison, image) in &self.captures {
            for (i, chunk) in image.chunks(256).enumerate() {
                writeln!(
                    rows,
                    "GT710_GR_IMAGE {} {} {}",
                    poison,
                    i * 256,
                    super::hex(chunk)
                )
                .map_err(|_| "GR image formatting")?;
            }
        }
        if rows.is_empty() {
            return Err("GR images missing");
        }
        let estimate = super::bulk::used()
            .checked_add(rows.len())
            .ok_or("GR artifact overflow")?;
        let bytes = rows.len();
        let digest = super::hash(rows.as_bytes());
        super::bulk::line(rows.trim_end_matches('\n').into())?;
        crate::serial_println!("GT710_GR_IMAGE_BULK {} {}", bytes, digest);
        Ok((bytes, estimate))
    }
    fn finish(mut self) -> Result<(), &'static str> {
        if let Some(graphics) = self.graphics.as_mut() {
            graphics.quiesce(self.regs, self.total)?;
        }
        if let Some(reader) = self.reader.as_mut() {
            reader.quiesce(self.regs, self.layout)?;
        }
        if let Some(reader) = self.reader.take() {
            reader.finish(self.regs, self.layout)?;
        }
        // FIFO is unbound, empty and restored before resetting FECS/GPCCS.
        ctxctl::reset(self.regs).map_err(hardware_error)?;
        unsafe {
            self.context.confirm_reset();
        }
        if let Some(graphics) = self.graphics.take() {
            graphics.finish(self.regs, self.layout, self.total)?;
        }
        self.context.retire(
            &mut self.layout.allocator,
            &mut super::channel::VmIo {
                regs: self.regs,
                total: self.total,
            },
        )?;
        for (_, a) in self.backing {
            self.layout.allocator.release(a)?;
        }
        Ok(())
    }
}
#[derive(Default)]
struct ShaderEvidence {
    rows: String,
    readbacks: Vec<String>,
}
impl ShaderEvidence {
    fn emit(&self, code_address: u64, r: &binding::Receipt, error: Option<&str>) -> Result<(), &'static str> {
        let assets: Vec<String> = [
            ("vertex.bin", &include_bytes!("../../../../assets/kepler/diagnostic-triangle-vertex.bin")[..]),
            ("vertex.sph", &include_bytes!("../../../../assets/kepler/diagnostic-triangle-vertex.sph")[..]),
            ("vertex.json", &include_bytes!("../../../../assets/kepler/diagnostic-triangle-vertex.json")[..]),
            ("vertex.asm", &include_bytes!("../../../../assets/kepler/diagnostic-triangle-vertex.asm")[..]),
            ("fragment.bin", &include_bytes!("../../../../assets/kepler/diagnostic-triangle-fragment.bin")[..]),
            ("fragment.sph", &include_bytes!("../../../../assets/kepler/diagnostic-triangle-fragment.sph")[..]),
            ("fragment.json", &include_bytes!("../../../../assets/kepler/diagnostic-triangle-fragment.json")[..]),
            ("fragment.asm", &include_bytes!("../../../../assets/kepler/diagnostic-triangle-fragment.asm")[..]),
        ].into_iter().map(|(name, bytes)| format!("\"{}\":\"{}\"", name, super::hash(bytes))).collect();
        let release = r.observation.as_ref().map(|o| format!(
            "{{\"target\":{},\"method\":6924,\"operation\":268496912,\"epoch\":{},\"nonce\":{},\"observed\":{},\"observed_epoch\":{},\"consumed\":{},\"accepted\":{},\"elapsed_us\":{}}}",
            r.target, r.epoch, r.nonce, o.value, o.epoch, o.consumed, o.accepted, o.elapsed_us,
        )).unwrap_or_else(|| "null".into());
        let error = error.map(|s| format!("\"{}\"", s)).unwrap_or_else(|| "null".into());
        let digest = super::hash(self.rows.as_bytes());
        super::bulk::line(self.rows.trim_end_matches('\n').into())?;
        crate::serial_println!("GT710_SHADER_BULK {} {}", self.rows.len(), digest);
        crate::serial_println!(
            "GT710_SHADER {{\"schema\":1,\"assets\":{{{}}},\"class_id\":41367,\"subchannel\":{},\"code_address\":{},\"segment_bytes\":{},\"offsets\":[{},{}],\"gprs\":[{},{}],\"readbacks\":[{}],\"release\":{},\"trap_registers\":{:?},\"trap_before\":{:?},\"trap_after\":{:?},\"error\":{},\"draw\":\"pending-7.4\"}}",
            assets.join(","), channel::SUBCHANNEL, code_address, shaders::SEGMENT_SIZE,
            shaders::VERTEX_OFFSET, shaders::FRAGMENT_OFFSET, shaders::VERTEX_GPRS, shaders::FRAGMENT_GPRS,
            self.readbacks.join(","), release, binding::TRAP_REGISTERS, r.before, r.after, error,
        );
        Ok(())
    }
}
/// Five draws on the session that bound the shader code. Transfers are checked
/// for byte equality; only the host scorer interprets rendered colours.
fn draw_probe(
    hardware: &mut Hardware<'_>, graphics: &mut channel::Session,
    code_address: u64, evidence: &mut DrawEvidence,
) -> Result<(), &'static str> {
    let target = graphics.allocate_draw(hardware, "draw-target", 64 * 256, 4096)?;
    let vertices = graphics.allocate_draw(hardware, "draw-vertices", 96, 4096)?;
    // Mesa 24.3.4 nvc0_screen.c:1274-1283 (MIT): poly_cache storage.
    // The context and backing list keep it pinned through FIFO drain and reset.
    let quarantine = graphics.allocate_draw(hardware, "draw-quarantine", 1 << 20, 1 << 17)?;
    let vertex_bytes: Vec<u8> = draw::VERTEX_WORDS.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut controls = alloc::vec![0; shaders::CODE_REGION_SIZE - shaders::SEGMENT_SIZE];
    for (offset, header, code) in [
        (shaders::CONSTANT_COLOUR_FRAGMENT_OFFSET, &shaders::CONSTANT_COLOUR_FRAGMENT_HEADER[..], &shaders::CONSTANT_COLOUR_FRAGMENT_CODE[..]),
        (shaders::DROP_COLOUR_VERTEX_OFFSET, &shaders::DROP_COLOUR_VERTEX_HEADER[..], &shaders::DROP_COLOUR_VERTEX_CODE[..]),
    ] {
        let start = offset - shaders::SEGMENT_SIZE;
        controls[start..start + header.len()].copy_from_slice(header);
        controls[start + header.len()..start + header.len() + code.len()].copy_from_slice(code);
    }
    for (va, data) in [(vertices.va, &vertex_bytes[..]), (code_address + shaders::SEGMENT_SIZE as u64, &controls[..])] {
        graphics.upload(hardware, va, data)?;
        let images = graphics.readback(hardware, va, data.len())?;
        if images[0] != data || images[1] != data {
            return Err("draw input CE readbacks differ from upload");
        }
    }
    let background = alloc::vec![0; 64 * 256];
    for (name, vp, fp) in [
        ("triangle", shaders::VERTEX_OFFSET, shaders::FRAGMENT_OFFSET),
        ("fragment-control", shaders::VERTEX_OFFSET, shaders::CONSTANT_COLOUR_FRAGMENT_OFFSET),
        ("after-fragment", shaders::VERTEX_OFFSET, shaders::FRAGMENT_OFFSET),
        ("vertex-control", shaders::DROP_COLOUR_VERTEX_OFFSET, shaders::FRAGMENT_OFFSET),
        ("after-vertex", shaders::VERTEX_OFFSET, shaders::FRAGMENT_OFFSET),
    ] {
        // Supply uniform background bytes for the CE transfer.
        graphics.upload(hardware, target.va, &background)?;
        let mut receipt = binding::Receipt {
            target: 0, epoch: 0, nonce: 0, before: [0; 4], after: [0; 4], observation: None,
        };
        let result = graphics.draw(hardware, draw::Draw {
            target_va: target.va, pitch: 256, width: 64, height: 64,
            vertex_va: vertices.va, quarantine_va: quarantine.va, vertex_program: vp as u32, fragment_program: fp as u32,
            report_va: 0, epoch: 0, nonce: 0,
        }, &mut receipt);
        evidence.draws.push(DrawCapture { name, receipt, images: None });
        result?;
        let images = graphics.readback(hardware, target.va, background.len())?;
        let capture = evidence.draws.last_mut().ok_or("draw capture absent")?;
        let equal = images[0] == images[1];
        capture.images = Some(images);
        if !equal { return Err("draw CE P/~P readbacks differ"); }
    }
    evidence.target = Some(target.va);
    Ok(())
}

/// Use the already published pair; keep both owners until the shared teardown.
fn contexts_probe(
    hardware: &mut Hardware<'_>, graphics: &mut channel::Session, code_address: u64,
) -> Result<(), &'static str> {
    let mut second = graphics.second.take().ok_or("context peer absent")?;
    let mut evidence = ContextsEvidence::default();
    let result = (|| {
        let mut requests = Vec::new();
        for (index, session) in [&mut *graphics, &mut *second].into_iter().enumerate() {
            // B has its own CODE_ADDRESS/stage bindings, installed once only.
            if index == 1 {
                // Populate slots 1-4 before bind_shaders publishes slot 5.
                session.execute(hardware)?;
                let mut receipt = binding::Receipt {
                    target: 0, epoch: 0, nonce: 0, before: [0; 4], after: [0; 4], observation: None,
                };
                session.bind_shaders(hardware, code_address, &mut receipt)?;
            }
            let target = session.allocate_draw(hardware, "context-target", 16384, 4096)?;
            let vertices = session.allocate_draw(hardware, "context-vertices", 96, 4096)?;
            let quarantine = session.allocate_draw(hardware, "context-quarantine", 1 << 20, 1 << 17)?;
            let words = if index == 0 { draw::VERTEX_WORDS } else { draw::FULL_VIEWPORT_VERTEX_WORDS };
            let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            session.upload(hardware, vertices.va, &bytes)?;
            let pair = session.readback(hardware, vertices.va, bytes.len())?;
            if pair[0] != bytes || pair[1] != bytes { return Err("context vertex upload mismatch"); }
            session.upload(hardware, target.va, &alloc::vec![0; 16384])?;
            if index == 1 {
                let mut bytes = Vec::from(&shaders::GRADIENT_FRAGMENT_HEADER[..]);
                bytes.extend_from_slice(shaders::GRADIENT_FRAGMENT_CODE);
                let va = code_address + u64::from(draw::GRADIENT_PROGRAM);
                session.upload(hardware, va, &bytes)?;
                let pair = session.readback(hardware, va, bytes.len())?;
                if pair[0] != bytes || pair[1] != bytes { return Err("context gradient upload mismatch"); }
            }
            let [instance, image, release, epoch] = session.context_storage();
            let physical = hardware.backing.iter().find(|(va, _)| *va == target.va)
                .ok_or("context target backing absent")?.1.range.start;
            let vertex_sha = super::hash(include_bytes!("../../../../assets/kepler/diagnostic-triangle-vertex.bin"));
            let fragment_sha = if index == 0 {
                super::hash(include_bytes!("../../../../assets/kepler/diagnostic-triangle-fragment.bin"))
            } else { super::hash(shaders::GRADIENT_FRAGMENT_CODE) };
            evidence.channels.push(format!(
                "{{\"id\":\"{}\",\"instance\":{},\"image\":{},\"target\":{},\"release\":{},\"epoch\":{},\"vertex_sha256\":\"{}\",\"fragment_sha256\":\"{}\"}}",
                if index == 0 { "a" } else { "b" }, instance, image, physical, release, epoch, vertex_sha, fragment_sha));
            requests.push(draw::Draw {
                target_va: target.va, pitch: 256, width: 64, height: 64,
                vertex_va: vertices.va, quarantine_va: quarantine.va,
                vertex_program: shaders::VERTEX_OFFSET as u32,
                fragment_program: if index == 0 { shaders::FRAGMENT_OFFSET as u32 } else { draw::GRADIENT_PROGRAM },
                report_va: 0, epoch: 0, nonce: 0,
            });
        }
        contexts_sequence(hardware, graphics, &mut second, [requests[0], requests[1]], &mut evidence)
    })();
    // Even an upload/submission failure must return the peer to pair teardown.
    graphics.second = Some(second);
    let emitted = evidence.emit(result.as_ref().err().copied());
    result.and(emitted)
}

/// Alternate saved contexts, then reject one-shot controls and redraw cleanly.
fn contexts_sequence(
    hardware: &mut Hardware<'_>, a: &mut channel::Session, b: &mut channel::Session,
    requests: [draw::Draw; 2], evidence: &mut ContextsEvidence,
) -> Result<(), &'static str> {
    for (index, name, variant, full) in [
        (0, "a-v1", "v1", true), (1, "b-gradient", "gradient", true),
        (0, "a-v2", "v2", false), (1, "b-restored", "gradient", false),
        (0, "suppress", "v2", false), (0, "after-suppress", "v2", false),
        (1, "corrupt", "gradient", false), (1, "after-corrupt", "gradient", false),
    ] {
        let (active, peer) = if index == 0 { (&mut *a, &mut *b) } else { (&mut *b, &mut *a) };
        let request = requests[index];
        // Clear stale pixels so restored draws must actually render, and the
        // suppressed control exposes the background.
        if matches!(name, "a-v2" | "b-restored" | "suppress" | "corrupt") {
            active.upload(hardware, request.target_va, &alloc::vec![0; 16384])?;
        }
        if name == "a-v2" {
            let bytes: Vec<u8> = draw::V2_VERTEX_WORDS.iter().flat_map(|w| w.to_le_bytes()).collect();
            active.upload(hardware, request.vertex_va, &bytes)?;
        }
        let peer_images = peer.readback(hardware, requests[1-index].target_va, 16384)?;
        if peer_images[0] != peer_images[1] { return Err("context peer P/~P mismatch before draw"); }
        let peer_before = peer_images.each_ref().map(|image| super::hash(image));
        let mut receipt = binding::Receipt {
            target: 0, epoch: 0, nonce: 0, before: [0; 4], after: [0; 4], observation: None,
        };
        let mut current = [0; 2];
        let result = active.context_draw(hardware, request, full, name == "suppress", &mut receipt, &mut current);
        evidence.frames.push(ContextFrame {
            name, channel: if index == 0 { "a" } else { "b" }, variant, current,
            receipt, peer_before, peer_after: [String::new(), String::new()], images: None, accepted: false,
        });
        result?;
        // context_draw reads CHAN_ADDR after release, before these CE transfers.
        if name == "corrupt" {
            // The full gradient covers the target, so any epoch-chosen pixel
            // differs from the background. Only this four-byte CE upload is
            // injected; the next B draw uses its saved state with no control.
            let offset = (active.context_storage()[3] % 4096) * 4;
            active.upload(hardware, request.target_va + offset, &0u32.to_le_bytes())?;
        }
        let images = active.readback(hardware, request.target_va, 16384)?;
        let peer_after = peer.readback(hardware, requests[1-index].target_va, 16384)?;
        let frame = evidence.frames.last_mut().ok_or("context frame absent")?;
        frame.peer_after = peer_after.each_ref().map(|image| super::hash(image));
        let same = images[0] == images[1];
        frame.accepted = same && context_pixels(&images[0], variant);
        frame.images = Some(images);
        if peer_after != peer_images { return Err("context peer changed across draw"); }
        if !same { return Err("context target P/~P mismatch"); }
        let control = name == "suppress" || name == "corrupt";
        if frame.accepted == control { return Err("context pixel oracle unexpected decision"); }
    }
    Ok(())
}

/// Present the final clean target without teaching EVO another method.
fn present_probe(
    hardware: &mut Hardware<'_>, graphics: &mut channel::Session,
    evo: &mut super::native::Evo, evidence: &mut DrawEvidence,
) -> Result<(), &'static str> {
    use core::fmt::Write;
    let console = hardware.console;
    let second = evo.surfaces[1];
    let (width,height,pitch) = (console.width,console.height,console.pitch);
    let (x,y,background) = (32usize,32usize,0xff182028u32);
    // Geometry is bounded before touching the inactive scanout.
    if width < x+64 || height < y+64 || pitch < width*4 || pitch%4 != 0
        || pitch.checked_mul(height).is_none_or(|n| n as u64 > second.size || n > 16 << 20) {
        return Err("presentation exceeds private scanout");
    }
    let result = (|| {
        let target = evidence.target.ok_or("clean draw target absent")?;
        let receipt = &evidence.draws.last().ok_or("clean draw receipt absent")?.receipt;
        let scanout = graphics.map_scanout(hardware)?;
        {
            let mut p = memory::Pramin::new(hardware.regs,hardware.total);
            for offset in (0..pitch*height).step_by(4) {
                p.write(second.start+offset as u64,background)?;
            }
        }
        super::modeset::bar_flush(hardware.regs)?;
        graphics.copy_rect(hardware,super::copy::Rect {
            src: target, dst: scanout+(y*pitch+x*4) as u64,
            width: 256, height: 64, src_pitch: 256, dst_pitch: pitch as u32,
        })?;
        let snapshot = evo.flip(hardware.regs,second.start)?;
        let images = graphics.readback(hardware,scanout,pitch*height)?;
        if images[0] != images[1] { return Err("present CE P/~P readbacks differ"); }
        let mut rows = String::new();
        let mut readbacks = Vec::new();
        for (poison,image) in [85,170].into_iter().zip(&images) {
            for (row,bytes) in image.chunks_exact(pitch).enumerate() {
                let mut runs: Vec<(usize,u32)> = Vec::new();
                for pixel in bytes.chunks_exact(4) {
                    let value = u32::from_le_bytes(pixel.try_into().unwrap());
                    if let Some((count,_)) = runs.last_mut().filter(|(_,word)| *word == value) {
                        *count += 1;
                    } else { runs.push((1,value)); }
                }
                write!(rows,"GT710_DRAW_IMAGE present {} {} ",poison,row).map_err(|_| "present row formatting")?;
                for (i,(count,word)) in runs.iter().enumerate() {
                    if i != 0 { rows.push(','); }
                    write!(rows,"{count:x}:{word:08x}").map_err(|_| "present run formatting")?;
                }
                rows.push('\n');
            }
            readbacks.push(format!("{{\"case\":\"present\",\"poison\":{},\"bytes\":{},\"sha256\":\"{}\"}}",
                poison,image.len(),super::hash(image)));
        }
        evidence.present_rows = rows;
        evidence.present_readbacks = readbacks;
        Ok((snapshot,receipt.epoch,receipt.nonce))
    })();
    // An error can follow a consumed UPDATE, so restore with the advanced PUT
    // even when the presentation flip or readback failed. Check the binding too.
    let restore = evo.flip(hardware.regs,evo.surfaces[0].start);
    let binding = super::native::console_binding(console);
    restore?;
    binding?;
    let (s,epoch,nonce) = result?;
    crate::serial_println!(concat!("GT710_PRESENT {{\"schema\":1,\"width\":{},\"height\":{},\"pitch\":{},\"background\":{},\"x\":{},\"y\":{},",
        "\"second_address\":{},\"head\":{},\"active_register\":{},\"active_value\":{},\"active_address\":{},\"epoch\":{},\"nonce\":{},\"firmware_writes\":{},",
        "\"notifier_before\":{},\"notifier_after\":{},\"put\":{},\"get\":{},\"raster_before\":{},\"raster_after\":{},\"underflow\":{},\"errors\":{},\"power\":{},\"link\":{},\"console_restored\":true}}"),
        width,height,pitch,background,x,y,second.start,evo.head,s.active_register,s.active_value,u64::from(s.active_value)<<8,
        epoch,nonce,crate::firmware_writes::count()-console.firmware_baseline,
        s.notifier_before,s.notifier_after,s.put,s.get,s.raster_before,s.raster_after,s.underflow,s.errors,s.power,s.link);
    Ok(())
}

#[derive(Default)]
struct ContextsEvidence {
    channels: Vec<String>,
    frames: Vec<ContextFrame>,
}
struct ContextFrame {
    name: &'static str,
    channel: &'static str,
    variant: &'static str,
    current: [u32; 2],
    receipt: binding::Receipt,
    peer_before: [String; 2],
    peer_after: [String; 2],
    images: Option<[Vec<u8>; 2]>,
    accepted: bool,
}

/// Check read-back only: the GPU, not this verifier, supplies actual pixels.
/// Coverage hashes and literal dyadic samples come from the u1 provenance.
fn context_pixels(image: &[u8], variant: &str) -> bool {
    if image.len() != 16384 { return false; }
    if variant == "gradient" {
        return image.chunks_exact(4).enumerate().all(|(i, pixel)| {
            let (x, y) = ((i % 64) as u8, (i / 64) as u8);
            pixel == [4*x, 4*y, 2*(x+y), 255]
        });
    }
    let (coverage, samples): (&str, &[(usize, usize, [u8; 4])]) = if variant == "v1" {
        ("67f58e6f204c69e7ddebf6057c92d5fd19549be11f42067405ff9065ab6b28ba", &[
            (15,14,[126,131,63,255]), (14,15,[124,138,60,255]), (15,15,[126,138,56,255]),
            (12,16,[120,145,61,255]), (13,16,[122,145,57,255]), (14,16,[124,145,53,255]),
        ])
    } else if variant == "v2" {
        ("0d8340d0e10f22323048b185495fbe1f992414cc54906fe7e89df9cd46efdba0", &[
            (52,7,[126,127,83,255]), (52,8,[124,123,87,255]), (51,9,[126,113,93,255]),
            (52,9,[122,119,91,255]), (53,9,[118,125,89,255]), (51,10,[124,109,97,255]),
        ])
    } else { return false; };
    let mut mask = Vec::with_capacity(4096);
    for pixel in image.chunks_exact(4) {
        match pixel[3] {
            0 if pixel == [0; 4] => mask.push(0),
            255 => mask.push(1),
            _ => return false,
        }
    }
    super::hash(&mask) == coverage && samples.iter().all(|(x,y,bgra)| {
        image[(y*64+x)*4..(y*64+x+1)*4] == *bgra
    })
}
impl ContextsEvidence {
    fn emit(&self, error: Option<&str>) -> Result<(), &'static str> {
        use core::fmt::Write;
        let mut rows = String::new();
        let mut readbacks = Vec::new();
        let mut frames = Vec::new();
        for frame in &self.frames {
            if let Some(images) = &frame.images {
                for (poison, image) in [85, 170].into_iter().zip(images) {
                    for (y, row) in image.chunks_exact(256).enumerate() {
                        let mut runs = Vec::new();
                        for pixel in row.chunks_exact(4) {
                            let word = u32::from_le_bytes(pixel.try_into().map_err(|_| "context pixel length")?);
                            match runs.last_mut() {
                                Some((count, last)) if *last == word => *count += 1,
                                _ => runs.push((1u32, word)),
                            }
                        }
                        let encoded = runs.iter().map(|(count, word)| format!("{:x}:{:08x}", count, word)).collect::<Vec<_>>().join(",");
                        writeln!(rows, "GT710_CTX_IMAGE {} {} {} {}", frame.name, poison, y, encoded)
                            .map_err(|_| "context row formatting")?;
                    }
                    readbacks.push(format!("{{\"case\":\"{}\",\"poison\":{},\"bytes\":{},\"sha256\":\"{}\"}}",
                        frame.name, poison, image.len(), super::hash(image)));
                }
            }
            let r = &frame.receipt;
            let receipt = r.observation.as_ref().map(|o| format!(
                "{{\"target\":{},\"epoch\":{},\"nonce\":{},\"observed_epoch\":{},\"observed\":{},\"consumed\":{},\"accepted\":{}}}",
                r.target, r.epoch, r.nonce, o.epoch, o.value, o.consumed, o.accepted)).unwrap_or_else(|| "null".into());
            frames.push(format!(
                "{{\"case\":\"{}\",\"channel\":\"{}\",\"variant\":\"{}\",\"current_before\":{},\"current_after\":{},\"trap_before\":{:?},\"trap_after\":{:?},\"peer_before\":{:?},\"peer_after\":{:?},\"accepted\":{},\"receipt\":{}}}",
                frame.name, frame.channel, frame.variant, frame.current[0], frame.current[1],
                r.before, r.after, frame.peer_before, frame.peer_after, frame.accepted, receipt));
        }
        let estimate = super::bulk::used().checked_add(rows.len()).ok_or("context bulk overflow")?;
        if estimate > 2 << 20 { return Err("context bulk exceeds 2 MiB"); }
        if !rows.is_empty() {
            let digest = super::hash(rows.as_bytes());
            super::bulk::line(rows.trim_end_matches('\n').into())?;
            crate::serial_println!("GT710_CTX_BULK {} {}", rows.len(), digest);
        }
        let error = error.map(|e| format!("{:?}", e)).unwrap_or_else(|| "null".into());
        crate::serial_println!(
            "GT710_CONTEXTS {{\"schema\":1,\"class_id\":41367,\"current_register\":4233984,\"trap_registers\":{:?},\"channels\":[{}],\"frames\":[{}],\"readbacks\":[{}],\"error\":{}}}",
            binding::TRAP_REGISTERS, self.channels.join(","), frames.join(","), readbacks.join(","), error);
        Ok(())
    }
}


#[derive(Default)]
struct DrawEvidence {
    draws: Vec<DrawCapture>,
    trap_capture: Option<binding::TrapCapture>,
    target: Option<u64>,
    present_rows: String,
    present_readbacks: Vec<String>,
}
struct DrawCapture {
    name: &'static str,
    receipt: binding::Receipt,
    images: Option<[Vec<u8>; 2]>,
}
impl DrawEvidence {
    fn emit(&self, error: Option<&str>) -> Result<(), &'static str> {
        use core::fmt::Write;
        let mut rows = String::new();
        let mut readbacks = Vec::new();
        let mut releases = Vec::new();
        let mut before = [0; 4];
        let mut after = [0; 4];
        // Preserve unit 4's wire order; the execution order is fragment first.
        for name in ["triangle", "vertex-control", "after-vertex", "fragment-control", "after-fragment"] {
            let Some(capture) = self.draws.iter().find(|c| c.name == name) else { continue; };
            let r = &capture.receipt;
            for i in 0..4 { before[i] |= r.before[i]; after[i] |= r.after[i]; }
            if let Some(images) = &capture.images {
                for (poison, image) in [85, 170].into_iter().zip(images) {
                    for (i, chunk) in image.chunks(256).enumerate() {
                        writeln!(rows, "GT710_DRAW_IMAGE {} {} {} {}", name, poison, i * 256, super::hex(chunk))
                            .map_err(|_| "draw image formatting")?;
                    }
                    readbacks.push(format!("{{\"case\":\"{}\",\"poison\":{},\"bytes\":{},\"sha256\":\"{}\"}}",
                        name, poison, image.len(), super::hash(image)));
                }
            }
        }
        for capture in &self.draws {
            let r = &capture.receipt;
            if let Some(o) = &r.observation {
                releases.push(format!(
                    "{{\"target\":{},\"epoch\":{},\"nonce\":{},\"observed\":{},\"observed_epoch\":{},\"consumed\":{},\"accepted\":{},\"elapsed_us\":{},\"trap_before\":{:?},\"trap_after\":{:?}}}",
                    r.target, r.epoch, r.nonce, o.value, o.epoch, o.consumed, o.accepted, o.elapsed_us, r.before, r.after));
            }
        }
        rows.push_str(&self.present_rows);
        readbacks.extend(self.present_readbacks.iter().cloned());
        // Retain headroom for pending golden-context rows in the shared buffer.
        let estimate = super::bulk::used().checked_add(rows.len()).and_then(|n| n.checked_add(512 * 1024))
            .ok_or("draw artifact estimate overflow")?;
        if estimate >= 2 << 20 { return Err("draw artifact estimate exceeds 2 MiB"); }
        if !rows.is_empty() {
            let digest = super::hash(rows.as_bytes());
            super::bulk::line(rows.trim_end_matches('\n').into())?;
            crate::serial_println!("GT710_DRAW_BULK {} {}", rows.len(), digest);
        }
        let trap_capture = self.trap_capture.as_ref().map(|c| format!(
            "{{\"registers\":{:?},\"values\":{:?},\"status\":{:?}}}", c.registers, c.values, c.status))
            .unwrap_or_else(|| "null".into());
        let error = error.map(|e| format!("{:?}", e)).unwrap_or_else(|| "null".into());
        crate::serial_println!(
            "GT710_DRAW {{\"schema\":1,\"class_id\":41367,\"readbacks\":[{}],\"releases\":[{}],\"trap_registers\":{:?},\"trap_before\":{:?},\"trap_after\":{:?},\"trap_capture\":{},\"error\":{}}}",
            readbacks.join(","), releases.join(","), binding::TRAP_REGISTERS, before, after, trap_capture, error);
        Ok(())
    }
}
struct ShaderUpload<'a, 'b> {
    hardware: &'a mut Hardware<'b>,
    buffer: context::Buffer,
    evidence: &'a mut ShaderEvidence,
}
impl shaders::UploadIo for ShaderUpload<'_, '_> {
    type Error = &'static str;
    fn poison(&mut self, byte: u8) -> Result<(), Self::Error> {
        self.hardware.fill(self.buffer, &alloc::vec![byte; shaders::SEGMENT_SIZE]).map_err(|e| e.reason)
    }
    fn upload(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.hardware.reader.as_mut().ok_or("shader CE reader absent")?.upload(
            self.hardware.regs, self.hardware.total, self.buffer.va, bytes)
    }
    fn readback(&mut self, case: shaders::UploadCase, expected: &[u8]) -> Result<(), Self::Error> {
        let images = self.hardware.reader.as_mut().ok_or("shader CE reader absent")?.image(
            self.hardware.regs, self.hardware.total, self.buffer.va, expected.len())?;
        for (index, image) in images.iter().enumerate() {
            use core::fmt::Write;
            for (i, chunk) in image.chunks(256).enumerate() {
                writeln!(self.evidence.rows, "GT710_SHADER_IMAGE {:?} {} {} {}", case, [85, 170][index], i * 256, super::hex(chunk))
                    .map_err(|_| "shader image formatting")?;
            }
            self.evidence.readbacks.push(format!(
                "{{\"case\":\"{:?}\",\"poison\":{},\"bytes\":{},\"sha256\":\"{}\"}}",
                case, [85, 170][index], image.len(), super::hash(image),
            ));
            if image != expected { return Err("shader CE upload bytes differ"); }
        }
        Ok(())
    }
}
fn layout_units(layout: &context::SaveLayout) -> String {
    let units: Vec<String> = layout
        .units
        .iter()
        .map(|u| {
            let strands: Vec<String> = u
                .strands
                .iter()
                .map(|s| format!("{{\"base\":{},\"words\":{}}}", s.base, s.words))
                .collect();
            format!(
                "{{\"controller\":{},\"mmio_base\":{},\"mmio_words\":{},\"strands\":[{}]}}",
                u.controller,
                u.mmio_base,
                u.mmio_words,
                strands.join(",")
            )
        })
        .collect();
    format!("[{}]", units.join(","))
}
impl context::Io for Hardware<'_> {
    fn allocate(&mut self, spec: context::Spec) -> Result<context::Buffer, context::Error> {
        let a = self.layout.allocator.allocate(
            spec.bytes,
            spec.align.max(4096),
            memory::allocator::Kind::Engine,
        )?;
        let va = self.next_va;
        self.next_va += a.range.size + 4096;
        self.backing.push((va, a));
        self.context.map(
            &mut self.layout.allocator,
            &mut super::channel::VmIo {
                regs: self.regs,
                total: self.total,
            },
            va,
            a,
            super::vm::Flags {
                volatile: true,
                ..super::vm::Flags::default()
            },
        )?;
        let mut p = memory::Pramin::new(self.regs, self.total);
        for offset in (0..a.range.size).step_by(4) {
            p.write(a.range.start + offset, 0)?;
        }
        Ok(context::Buffer { spec, va })
    }
    fn patch(&mut self, b: context::Buffer, pairs: &[[u32; 2]]) -> Result<(), context::Error> {
        if pairs.len() * 8 > b.spec.bytes as usize {
            return Err("GR patch list exceeds backing".into());
        }
        let mut data = alloc::vec![0;b.spec.bytes as usize];
        for (i, p) in pairs.iter().enumerate() {
            for j in 0..2 {
                data[i * 8 + j * 4..i * 8 + j * 4 + 4].copy_from_slice(&p[j].to_le_bytes());
            }
        }
        self.fill(b, &data)
    }
    fn point(&mut self, b: context::Buffer) -> Result<(), context::Error> {
        if !self.admitted {
            unsafe {
                self.context.admit_device()?;
            }
            self.admitted = true;
        }
        super::inventory::words(
            self.regs,
            self.total,
            self.context.space().instance.range.start + 0x210,
            &[b.va as u32 | 4, (b.va >> 32) as u32],
        )?;
        super::modeset::bar_flush(self.regs)?;
        Ok(())
    }
    fn poison(&mut self, b: context::Buffer, byte: u8) -> Result<(), context::Error> {
        self.fill(b, &alloc::vec![byte;b.spec.bytes as usize])
    }
    fn request(&mut self, method: u32, poison: u32) -> Result<ctxctl::Reply, context::Error> {
        let instance = 0x80000000 | (self.context.space().instance.range.start >> 12) as u32;
        self.controller
            .request(self.regs, method, instance, poison)
            .map_err(|e| hardware_error(e).into())
    }
    fn readback(&mut self, b: context::Buffer) -> Result<[Vec<u8>; 2], context::Error> {
        if self.reader.is_none() {
            self.reader = Some(super::ce::readback::Reader::new(
                self.regs,
                self.device,
                self.layout,
                self.console,
                &self.backing,
            )?);
        }
        let images = self.reader.as_mut().unwrap().image(
            self.regs,
            self.total,
            b.va,
            b.spec.bytes as usize,
        )?;
        if b.spec.name == "golden-55" || b.spec.name == "golden-aa" {
            self.captures.push((
                if b.spec.name == "golden-55" { 85 } else { 170 },
                images[0].clone(),
            ));
        }
        Ok(images)
    }
    fn capture_layout(&mut self) -> Result<context::SaveLayout, context::Error> {
        let layout = context::SaveLayout::capture(self)?;
        crate::serial_println!("GT710_GR_LAYOUT {{\"units\":{}}}", layout_units(&layout));
        Ok(layout)
    }
    fn runnable(
        &mut self,
        b: context::Buffer,
        image: &[u8],
        header: [u32; 2],
    ) -> Result<[u32; 2], context::Error> {
        let mut data = image.to_vec();
        data[..256].fill(0);
        data[..4].copy_from_slice(&header[0].to_le_bytes());
        data[4..8].copy_from_slice(&header[1].to_le_bytes());
        self.fill(b, &data)?;
        let [first, second] = self.readback(b)?;
        if first != data || second != data {
            return Err("runnable context CE bytes differ".into());
        }
        self.runnable_header = first[..256].to_vec();
        Ok([
            u32::from_le_bytes(first[..4].try_into().unwrap()),
            u32::from_le_bytes(first[4..8].try_into().unwrap()),
        ])
    }
}
