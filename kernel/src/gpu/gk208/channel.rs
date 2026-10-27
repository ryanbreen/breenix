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
/*
 * Copyright 2016 Red Hat Inc.
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
 * Copyright 2016 Red Hat Inc.
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
/*******************************************************************************
    Copyright (c) 2020, NVIDIA CORPORATION. All rights reserved.

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
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
    THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
    FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
    DEALINGS IN THE SOFTWARE.

*******************************************************************************/
//! One boot-CPU-only A06F channel. IRQ delivery remains masked during this
//! bounded bootstrap: no decode, allocation or recovery runs in a top half.
//! Host semaphore completion certifies PFIFO execution only, not CE/PGRAPH.
use super::{
    inventory::{stats, words},
    lifetime::{Consumer, Context},
    memory::{self, allocator::Kind},
    modeset::{self, Io},
    vm::{self, Flags},
    vm_control, Registers,
};
use crate::{arch_impl::x86_64::timer, drivers::pci};
use alloc::{format, vec::Vec};
const VA: u64 = 0x100000;
const USERD_OFFSET: u64 = (8 << 20) - 4096;
struct VmIo<'a> {
    regs: &'a mut Registers,
    total: u64,
}
impl vm::Io for VmIo<'_> {
    fn read(&mut self, a: u64) -> Result<u64, &'static str> {
        let mut p = memory::Pramin::new(self.regs, self.total);
        Ok(u64::from(p.read(a)?) | u64::from(p.read(a + 4)?) << 32)
    }
    fn write(&mut self, a: u64, v: u64) -> Result<(), &'static str> {
        words(self.regs, self.total, a, &[v as u32, (v >> 32) as u32])
    }
    fn flush(&mut self) -> Result<(), &'static str> {
        modeset::bar_flush(self.regs)
    }
    fn invalidate(&mut self, pd: u64) -> Result<u32, &'static str> {
        vm_control::invalidate(self.regs, pd)
    }
    fn register(&mut self, r: usize) -> u32 {
        self.regs.read(r)
    }
}
fn read(regs: &mut Registers, total: u64, a: u64) -> Result<u32, &'static str> {
    memory::Pramin::new(regs, total).read(a)
}
fn elapsed(begin: u64) -> u64 {
    timer::rdtsc().wrapping_sub(begin) / (timer::frequency_hz() / 1_000_000)
}
fn wait(
    regs: &mut Registers,
    r: usize,
    mask: u32,
    value: u32,
    budget: u64,
) -> Result<(), &'static str> {
    let begin = timer::rdtsc();
    while regs.read(r) & mask != value {
        if elapsed(begin) >= budget {
            return Err("PFIFO bounded wait expired");
        }
        regs.delay_us(10);
    }
    Ok(())
}
fn accepted(epoch: u64, current: u64, observed: u32, expected: u32, get: u32, put: u32) -> bool {
    epoch == current && observed == expected && get == put
}
/// Run once before SMP and before publishing any clients. On any uncertain
/// result Context's existing residency owner keeps tables and backing pinned.
pub fn probe(
    regs: &mut Registers,
    device: &pci::Device,
    layout: &mut memory::Layout,
    console: &super::ConsoleSurface,
) -> Result<(), &'static str> {
    let baseline = layout.allocator.stats();
    let total = baseline.reserved + baseline.charged + baseline.free;
    if timer::frequency_hz() < 1_000_000 {
        return Err("channel requires calibrated clock");
    }
    // Refuse firmware-owned channels before constructing owned RAMFC/USERD.
    for id in 0..1024 {
        if regs.read(0x800000 + id * 8) != 0 {
            return Err("inherited FIFO channel remains bound");
        }
    }
    let top: Vec<u32> = (0..64).map(|i| regs.read(0x22700 + i * 4)).collect();
    let (mut eng, mut run) = (None, None);
    let mut selected = None;
    for &word in &top {
        match word & 3 {
            0 => continue,
            2 => {
                if word & 32 != 0 {
                    eng = Some((word >> 26) & 15);
                }
                if word & 16 != 0 {
                    run = Some((word >> 21) & 15);
                }
            }
            _ => (),
        }
        if word & 0x80000000 == 0 {
            if word & 3 == 3 && matches!((word & 0x7ffffffc) >> 2, 1..=3) && selected.is_none() {
                if let (Some(e), Some(r)) = (eng, run) {
                    selected = Some((e, r));
                }
            }
            eng = None;
            run = None;
        }
    }
    let (engine, runlist) = selected.ok_or("no enumerated copy-engine runlist")?;
    let enabled = regs.read(0x204);
    regs.write(0x204, u32::MAX);
    let implemented = regs.read(0x204);
    regs.write(0x204, enabled);
    let pbdmas: Vec<[u32; 2]> = (0..32)
        .filter(|i| implemented & (1 << i) != 0)
        .map(|i| [i as u32, regs.read(0x2390 + i * 4)])
        .collect();
    let pbdma = pbdmas
        .iter()
        .find(|p| p[1] & (1 << runlist) != 0)
        .ok_or("runlist has no enabled PBDMA")?[0] as usize;
    crate::serial_println!(
        "GT710_CHANNEL_STAGE discovery engine={} runlist={} pbdmas={:?} top={:?}",
        engine,
        runlist,
        pbdmas,
        top
    );
    let mut io = VmIo { regs, total };
    let mut context = Context::new(&mut layout.allocator, &mut io)?;
    let backing = layout.allocator.allocate(65536, 4096, Kind::Engine)?;
    context.map(
        &mut layout.allocator,
        &mut io,
        VA,
        backing,
        Flags {
            privileged: true,
            volatile: true,
            ..Flags::default()
        },
    )?;
    let instance = context.space().instance.range.start;
    let directory = context.space().directory.range.start;
    let ring = backing.range.start;
    let push = ring + 4096;
    let semaphore = ring + 8192;
    let userd = ring + 12288;
    let list = ring + 16384;
    for offset in (0..65536).step_by(4096) {
        words(regs, total, ring + offset, &[0; 1024])?;
    }
    // Copy the checked GK104 RAMFC format, selected by GK208's gk110 channel.
    for (offset, value) in [
        (0x08, userd as u32),
        (0x0c, 0),
        (0x10, 0xface),
        (0x30, 0xfffff902),
        (0x48, VA as u32),
        (0x4c, 9 << 16),
        (0x84, 0x20400000),
        (0x94, 0x30000fff),
        (0x9c, 0x100),
        (0xac, 0x1f),
        (0xe4, 0x20),
        (0xe8, 0),
        (0xb8, 0xf8000000),
        (0xf8, 0x10003080),
        (0xfc, 0x10000010),
    ] {
        words(regs, total, instance + offset, &[value])?;
    }
    words(regs, total, list, &[0, 0])?; // standalone channel, no hardware TSG
    let page = modeset::bar1_page(regs)?;
    let surface = layout.objects[0].range;
    let (_, displacement, _) =
        modeset::bar1_view(surface.start, surface.size, device.bars[1].size, page)?;
    if displacement + console.bytes > USERD_OFFSET as usize {
        return Err("no bounded BAR1 USERD slot");
    }
    let native_instance = layout
        .objects
        .iter()
        .find(|o| o.name == "instance")
        .ok_or("missing native instance")?
        .range
        .start;
    let bar_inst = native_instance + 0x2000;
    let bar_dir = native_instance + 0x3000;
    let pte = native_instance + 0x4000 + (USERD_OFFSET / 4096) * 8;
    if read(regs, total, pte)? != 0 || read(regs, total, pte + 4)? != 0 {
        return Err("BAR1 USERD slot already occupied");
    }
    words(regs, total, pte, &[((userd >> 8) | 1) as u32, 0])?;
    modeset::activate_bar1(regs, bar_inst, bar_dir)?;
    let old_userd = regs.read(0x2254);
    let old_intr = regs.read(0x2140);
    let bar_pointer = console.pointer - displacement;
    let publish = |regs: &mut Registers, put: u32| -> Result<(), &'static str> {
        modeset::bar_flush(regs)?;
        // SAFETY: the owned USERD page is installed in the existing WC BAR1
        // alias. PFIFO snoops this GPPut write at its programmed BAR1 offset.
        unsafe {
            ((bar_pointer + USERD_OFFSET as usize + 0x8c) as *mut u32).write_volatile(put);
        }
        crate::gpu_cache::drain_wc();
        let _ = regs.read(0x2254);
        Ok(())
    };
    device.enable_bus_master();
    regs.mask(0x200, 0x100, 0x100);
    regs.write(0x2140, 0); // polling bootstrap, no interrupt handler installed
    regs.write(0x204, implemented);
    regs.mask(0x2a04, 0xbfffffff, 0xbfffffff);
    for &[id, _] in &pbdmas {
        let b = 0x40000 + id as usize * 0x2000;
        regs.mask(b + 0x13c, 0x10000100, 0);
        regs.write(b + 0x108, u32::MAX);
        regs.write(b + 0x10c, 0);
        regs.write(b + 0x148, u32::MAX);
        regs.write(b + 0x14c, 0);
        regs.write(b + 0x12c, 0xf4240);
    }
    regs.write(0x2254, 0x10000000 | (USERD_OFFSET >> 12) as u32);
    regs.write(0x2100, u32::MAX);
    modeset::bar_flush(regs)?;
    // SAFETY: commands, USERD, ring and semaphore are retained by context;
    // no channel binding is visible until after admission.
    unsafe {
        context.admit_device()?;
    }
    regs.mask(0x800004, 0xf0000, runlist << 16);
    regs.write(0x800000, 0x80000000 | (instance >> 12) as u32);
    regs.mask(0x800004, 0x400, 0x400);
    regs.mask(0x2630, 1 << runlist, 0);
    regs.write(0x2270, (list >> 12) as u32);
    regs.write(0x2274, (runlist << 20) | 1);
    wait(regs, 0x2284 + runlist as usize * 8, 0x100000, 0, 2_000_000)?;
    let bind = regs.read(0x800000);
    let state = regs.read(0x800004);
    let epoch = timer::rdtsc() | 1;
    let seed = (epoch as u32) & 0x7fffffff | 0x100;
    let mut samples = Vec::new();
    for sequence in 1..=2u32 {
        let expected = seed.wrapping_add(sequence);
        let offset = u64::from(sequence - 1) * 64;
        let before = read(regs, total, semaphore)?;
        let get_before = read(regs, total, userd + 0x88)?;
        let ticket = context.hold(Consumer::Queued)?;
        // Incrementing A06F semaphore address/payload/release, then SET_REFERENCE.
        // 4-byte RELEASE with WFI enabled: host only, no CE commands admitted.
        let command = [
            0x20040004,
            0,
            (VA + 8192) as u32,
            expected,
            0x01000002,
            0x20010014,
            expected,
        ];
        words(regs, total, push + offset, &command)?;
        let addr = VA + 4096 + offset;
        words(
            regs,
            total,
            ring + u64::from(sequence - 1) * 8,
            &[addr as u32, ((command.len() as u32) << 10) | 0x100],
        )?;
        publish(regs, sequence)?;
        let begin = timer::rdtsc();
        let observed;
        loop {
            let v = read(regs, total, semaphore)?;
            let get = read(regs, total, userd + 0x88)?;
            if accepted(epoch, epoch, v, expected, get, sequence)
                && read(regs, total, userd + 0x48)? == expected
            {
                observed = v;
                break;
            }
            if elapsed(begin) >= 2_000_000 {
                crate::serial_println!("GT710_CHANNEL_FAILURE semaphore={} expected={} get={} put={} fifo={:#x} fault={:#x} pbdma0={:#x} pbdma1={:#x} state={:#x}",v,expected,get,sequence,regs.read(0x2100),regs.read(0x259c),regs.read(0x40108+pbdma*0x2000),regs.read(0x40148+pbdma*0x2000),regs.read(0x800004));
                return Err("host semaphore watchdog expired; storage retained");
            }
            regs.delay_us(10);
        }
        let reference = read(regs, total, userd + 0x48)?;
        samples.push(format!("{{\"sequence\":{},\"epoch\":{},\"expected\":{},\"before\":{},\"after\":{},\"get_before\":{},\"put\":{},\"get\":{},\"reference\":{}}}",sequence,epoch,expected,before,observed,get_before,sequence,read(regs,total,userd+0x88)?,reference));
        // SAFETY: matching native epoch, payload and GPGet observed for this host-only ticket.
        unsafe {
            context.finish_device(ticket)?;
        }
    }
    // A withheld GPFIFO entry cannot execute even though its payload is encoded.
    let expected = seed.wrapping_add(3);
    words(regs, total, semaphore, &[0])?;
    words(
        regs,
        total,
        push + 128,
        &[0x20040004, 0, (VA + 8192) as u32, expected, 0x01000002],
    )?;
    let begin = timer::rdtsc();
    let mut false_success = false;
    while elapsed(begin) < 10000 {
        false_success |= accepted(
            epoch,
            epoch,
            read(regs, total, semaphore)?,
            expected,
            read(regs, total, userd + 0x88)?,
            3,
        );
        regs.delay_us(10);
    }
    let withheld_elapsed = elapsed(begin);
    let withheld = read(regs, total, semaphore)?;
    let stale_value = seed.wrapping_add(2);
    words(regs, total, semaphore, &[stale_value])?;
    let stale_observed = read(regs, total, semaphore)?;
    let gpget = read(regs, total, userd + 0x88)?;
    let gpput = read(regs, total, userd + 0x8c)?;
    let stale = accepted(epoch - 1, epoch, stale_observed, stale_value, gpget, gpput);
    if false_success || stale {
        return Err("false channel completion");
    }
    let faults = regs.read(0x259c);
    let errors = [
        regs.read(0x40108 + pbdma * 0x2000),
        regs.read(0x40148 + pbdma * 0x2000),
    ];
    if faults != 0 || errors != [0, 0] {
        return Err("channel hardware fault; storage retained");
    }
    // Remove and preempt before clearing the binding. A failed step retains the
    // residency; no service power cycle is treated as a device drain receipt.
    regs.mask(0x2630, 1 << runlist, 1 << runlist);
    regs.mask(0x800004, 0x800, 0x800);
    regs.write(0x2634, 0);
    wait(regs, 0x2634, 0x100000, 0, 2_000_000)?;
    regs.write(0x2270, (list >> 12) as u32);
    regs.write(0x2274, runlist << 20);
    wait(regs, 0x2284 + runlist as usize * 8, 0x100000, 0, 2_000_000)?;
    wait(regs, 0x3080 + pbdma * 4, 0xe000, 0, 2_000_000)?;
    regs.write(0x800000, 0);
    let binding = regs.read(0x800000);
    let pending = regs.read(0x2284 + runlist as usize * 8) & 0x100000;
    let preempt = regs.read(0x2634) & 0x100000;
    if binding != 0 {
        return Err("channel unbind failed");
    }
    // SAFETY: channel stopped, removed from completed runlist, PBDMA idle,
    // preemption finished and binding cleared. There were only host methods.
    unsafe {
        context.confirm_drain();
    }
    regs.write(0x2254, old_userd);
    words(regs, total, pte, &[0, 0])?;
    modeset::activate_bar1(regs, bar_inst, bar_dir)?;
    regs.write(0x2140, old_intr);
    regs.write(0x204, enabled);
    context.retire(&mut layout.allocator, &mut VmIo { regs, total })?;
    layout.allocator.release(backing)?;
    if layout.allocator.stats() != baseline {
        return Err("channel reclamation mismatch");
    }
    crate::serial_println!("GT710_CHANNEL {{\"schema\":1,\"bdf\":\"{:02x}:{:02x}.{}\",\"chipset\":262,\"host_class\":41071,\"channel\":0,\"epoch\":{},\"instance\":{},\"directory\":{},\"userd\":{},\"ring\":{},\"push\":{},\"semaphore\":{},\"top\":{:?},\"pbdmas\":{:?},\"runlist\":{},\"engine\":{},\"bind\":{},\"state\":{},\"fault\":{},\"pbdma_errors\":{:?},\"samples\":[{}],\"withheld\":{{\"epoch\":{},\"expected\":{},\"observed\":{},\"get\":{},\"put\":{},\"elapsed_us\":{},\"accepted\":false}},\"stale\":{{\"epoch\":{},\"expected\":{},\"observed\":{},\"get\":{},\"put\":{},\"accepted\":false}},\"drain\":{{\"runlist_count\":0,\"pending\":{},\"preempt\":{},\"binding\":{}}},\"retained_on_failure\":true,\"firmware_writes\":{},\"ce\":\"pending-6.2\",\"vm_probes\":\"pending-6.2\",\"reset\":\"pending-6.5\",\"baseline\":{},\"after\":{}}}",
        device.bus,device.device,device.function,epoch,instance,directory,userd,ring,push,semaphore,top,pbdmas,runlist,engine,bind,state,faults,errors,samples.join(","),epoch,expected,withheld,gpget,gpput,withheld_elapsed,epoch-1,stale_value,stale_observed,gpget,gpput,pending,preempt,binding,crate::firmware_writes::count()-console.firmware_baseline,stats(baseline),stats(layout.allocator.stats()));
    Ok(())
}
