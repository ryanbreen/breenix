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
//! Native PGRAPH startup and golden context construction.
pub mod channel;
pub mod context;
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

/// Capture startup, golden context and one A197 channel for the raw scorer.
pub(super) fn probe(
    regs: &mut Registers,
    device: &crate::drivers::pci::Device,
    layout: &mut memory::Layout,
    console: &super::ConsoleSurface,
) -> Result<(), &'static str> {
    probe_static_inputs()?;
    ctxctl::reset(regs).map_err(hardware_error)?;
    let raw = [
        [0x409604, regs.read(0x409604)],
        [0x502608, regs.read(0x502608)],
        [0x500c30, regs.read(0x500c30)],
    ];
    // Retain the source words even if decoding refuses the topology, before
    // any firmware upload. No context or image bytes travel over serial.
    crate::serial_println!("GT710_GR_TOPOLOGY {:?}",raw);
    let topology = topology::decode(&raw)?;
    let baseline = layout.allocator.stats();
    let total = baseline.reserved + baseline.charged + baseline.free;
    // Scratch backing stays charged until the confirmed GR reset below.
    let scratch_bytes = 1u64 << if regs.read(0x100c80) & 1 != 0 { 16 } else { 17 };
    let mut mmu = [0; 2];
    let mut scratch = Vec::new();
    for index in 0..2 {
        let allocation =
            layout
                .allocator
                .allocate(scratch_bytes, 4096, memory::allocator::Kind::Engine)?;
        mmu[index] = allocation.range.start;
        scratch.push(allocation);
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
    let generated = context::generate(&mut hardware, &topology, ready.size);
    if let Err(error) = &generated {
        context_error(error);
    }
    let graphics = if let Ok(golden) = &generated {
        // CE read-back owns channel 0 and its USERD alias until drained.
        if let Some(reader) = hardware.reader.take() {
            reader.finish(hardware.regs, hardware.layout)?;
        }
        let mut graphics = channel::Session::new(&mut hardware, golden)?;
        graphics.execute(&mut hardware)?;
        let report = graphics.report();
        // Retain context and system backing until finish confirms GR reset.
        hardware.graphics = Some(graphics);
        Some(report)
    } else {
        None
    };
    // Preserve raw saves even when the byte check fails. No serial image dump.
    let artifact = hardware.emit_images();
    let runnable_bytes = hardware.runnable_header.clone();
    let firmware_hashes = hardware.controller.readback_sha256;
    hardware.finish()?;
    for allocation in scratch {
        layout.allocator.release(allocation)?;
    }
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
    let layout_words = golden.layout;
    let context_fields = format!("\"schema\":1,\"bdf\":\"{:02x}:{:02x}.{}\",\"chipset\":{},\"firmware\":{{{}}},\"status\":{:?},\"elapsed_us\":{},\"bound_us\":{},\"fecs_size\":{},\"queries\":[{}],\"withheld_start\":{{\"started\":{},\"ready\":{},\"poison\":{},\"reply\":{},\"accepted\":{}}},\"stale_reply\":{{\"method\":{},\"poison\":{},\"planted\":{},\"reply\":{},\"accepted\":{}}},\"topology_raw\":{:?},\"topology\":{{\"gpc_nr\":{},\"tpc_nr\":{:?},\"tpc_mask\":{:?}}},\"init_lists\":[{}],\"global_buffers\":[{}],\"golden_bytes\":{},\"saved_end\":{},\"save_layout\":{{\"gpc\":{},\"strands\":{},\"strand_base\":{},\"strand_words\":{}}},\"saves\":[{}],\"runnable_header\":{{\"count\":{},\"va\":{},\"words\":{:?},\"sha256\":\"{}\"}},\"image_bulk_bytes\":{},\"bulk_artifact_estimate\":{},\"baseline\":{},\"after\":{},\"firmware_writes\":{},\"draw\":\"pending-7.4\",\"shaders\":\"pending-7.3\",\"context_switch\":\"pending-7.5\"",
        device.bus,device.device,device.function,(regs.read(0) >> 20) & 0x1ff,
        firmware.join(","),ready.status,elapsed,ctxctl::BOUND_US,ready.size,queries.join(","),
        withheld.started,withheld.ready,withheld.poison,withheld.reply,withheld.accepted,
        stale.method,stale.poison,stale.planted,stale.reply,stale.accepted,
        raw,topology.gpc_nr,&topology.tpc_nr[..1],&topology.tpc_mask[..1],lists.join(","),buffers.join(","),
        ready.size,golden.saved_end,layout_words.gpc,layout_words.strands,layout_words.strand_base,layout_words.strand_words,
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
        // The graphics channel has left its runlist and drained. Reset stops
        // FECS/GPCCS before reclaiming either instance or its current image.
        ctxctl::reset(self.regs).map_err(hardware_error)?;
        unsafe {
            self.context.confirm_reset();
        }
        if let Some(reader) = self.reader.take() {
            reader.finish(self.regs, self.layout)?;
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
