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
 * Copyright 2021 Red Hat Inc.
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
 */
//! CE-only recovery on the native owner worker, outside interrupt/syscall paths.
//! GK104 channel stop/preempt/runlist and runl.c engine reset ordering (MIT).
use super::super::recovery_policy::{Action, Watchdog};
use super::*;

fn quarantine(regs: &mut Registers, c: &mut Channel, why: &'static str) {
    c.failure = Some(why);
    c.context.fail_eio();
    regs.mask(0x800004 + c.id as usize * 8, 0x800, 0x800);
}
fn budget_wait(regs: &mut Registers, start: u64, r: usize, mask: u32) -> Result<(), &'static str> {
    loop {
        if regs.read(r) & mask == 0 {
            return Ok(());
        }
        if elapsed(start) >= 100_000 {
            return Err("native reset budget exhausted; backing retained");
        }
        regs.delay_us(10);
    }
}
/// Stop scheduling A. The yielding host acquire may still name its backing, so
/// removal alone intentionally does NOT call confirm_drain or finish its ticket.
fn remove(
    regs: &mut Registers,
    total: u64,
    e: &Engine,
    a: &Channel,
    b: &Channel,
) -> Result<(), &'static str> {
    let start = timer::rdtsc();
    regs.mask(0x2630, 1 << e.run, 1 << e.run);
    let result = (|| {
        regs.write(0x2634, a.id);
        budget_wait(regs, start, 0x2634, 0x100000)?;
        words(regs, total, b.commands.range.start + RUNLIST, &[b.id, 0])?;
        modeset::bar_flush(regs)?;
        regs.write(0x2270, ((b.commands.range.start + RUNLIST) >> 12) as u32);
        regs.write(0x2274, (e.run << 20) | 1);
        regs.mask(0x2630, 1 << e.run, 0);
        budget_wait(regs, start, 0x2284 + e.run as usize * 8, 0x100000)?;
        regs.write(0x800000 + a.id as usize * 8, 0);
        if regs.read(0x800000 + a.id as usize * 8) != 0 {
            return Err("quarantined channel binding remains");
        }
        Ok(())
    })();
    // Restore scheduling after the closure, including its error returns.
    regs.mask(0x2630, 1 << e.run, 0);
    result
}
/// Local engine recovery retains B's RAMFC, identity, mappings and epoch. A CE
/// fault can stall the runlist; reset the enumerated CE before waiting on it.
fn fault_reset(
    regs: &mut Registers,
    total: u64,
    e: &Engine,
    a: &mut Channel,
    b: &Channel,
    mask: u32,
) -> Result<(u64, u32, u32), &'static str> {
    let start = timer::rdtsc();
    regs.mask(0x2630, 1 << e.run, 1 << e.run);
    let (disabled, enabled) = reset_engine(regs, e.base, e.reset)?;
    regs.write(0x259c, mask);
    regs.write(0x2100, 0x10000000);
    words(regs, total, b.commands.range.start + RUNLIST, &[b.id, 0])?;
    modeset::bar_flush(regs)?;
    regs.write(0x2270, ((b.commands.range.start + RUNLIST) >> 12) as u32);
    regs.write(0x2274, (e.run << 20) | 1);
    regs.write(0x262c, 1 << e.run);
    regs.mask(0x2630, 1 << e.run, 0);
    budget_wait(regs, start, 0x2284 + e.run as usize * 8, 0x100000)?;
    regs.write(0x2634, a.id);
    budget_wait(regs, start, 0x2634, 0x100000)?;
    budget_wait(regs, start, 0x3080 + e.pbdma * 4, 0xe000)?;
    budget_wait(regs, start, 0x2640 + e.engine as usize * 8, 0xc0000000)?;
    regs.write(0x800000 + a.id as usize * 8, 0);
    if disabled & (1 << e.reset) != 0
        || enabled & (1 << e.reset) == 0
        || regs.read(0x259c) != 0
        || regs.read(0x800000 + a.id as usize * 8) != 0
        || elapsed(start) > 100_000
    {
        return Err("local engine reset unconfirmed; backing retained");
    }
    Ok((elapsed(start), disabled, enabled))
}
// A receipt is issued only after these hardware observations agree. Calling
// this before reset deliberately takes the same refusal path with live backing.
fn transport_confirmed(regs: &mut Registers, e: &Engine, disabled: u32, enabled: u32) -> Result<(), &'static str> {
    if disabled & (1 << e.reset) != 0
        || enabled & (1 << e.reset) == 0
        || regs.read(0x259c) != 0
        || regs.read(0x800000) != 0
        || regs.read(0x800008) != 0
        || regs.read(0x2284 + e.run as usize * 8) & 0x100000 != 0
        || regs.read(0x3080 + e.pbdma * 4) & 0xe000 != 0
        || regs.read(0x2640 + e.engine as usize * 8) & 0xc0000000 != 0
    {
        return Err("transport reset unconfirmed; backing retained");
    }
    Ok(())
}
fn hold_check(layout: &mut memory::Layout, c: &Channel) -> Result<usize, &'static str> {
    if !layout.allocator.is_pinned(c.commands)?
        || !layout.allocator.is_pinned(c.dest)?
        || c.context.holds()[0] != 1
    {
        return Err("recovery lost device backing");
    }
    Ok(c.context.holds()[0])
}
fn compare(
    regs: &mut Registers,
    total: u64,
    e: &Engine,
    bar: usize,
    c: &mut Channel,
    seed: u32,
) -> Result<String, &'static str> {
    let mut digests = Vec::new();
    for poison in [0x55, 0xaa] {
        system_fill(c, poison);
        c.execute(
            regs,
            total,
            e,
            bar,
            rect(SOURCE, SYSTEM, SIZE as u32, 1, SIZE as u32, SIZE as u32),
        )?;
        let data = system_bytes(c);
        if data.iter().enumerate().any(|(n, v)| *v != pattern(n, seed)) {
            return Err("recovery complete poisoned compare differs");
        }
        digests.push(format!("\"{}\"", super::super::hash(&data)));
    }
    Ok(format!("[{}]", digests.join(",")))
}
fn gate(
    regs: &mut Registers,
    total: u64,
    bar: usize,
    c: &mut Channel,
) -> Result<(Ticket, Request), &'static str> {
    let ticket = c.context.hold(Consumer::Queued)?;
    let sequence = c.queue.next()?;
    let command = gated_copy(c, sequence)?;
    c.push(regs, total, bar, &command, true)?;
    Ok((ticket, c.request(sequence, c.dest.range.start, 1)))
}
fn rejected(
    regs: &mut Registers,
    total: u64,
    bar: usize,
    c: &mut Channel,
) -> Result<bool, &'static str> {
    let before = c.put;
    if c.submit(regs, total, bar, rect(SOURCE, DEST, 1, 1, 1, 1), true)
        .is_ok()
        || c.put != before
    {
        return Err("quarantined channel admitted new work");
    }
    Ok(true)
}
fn pause() -> Result<(), &'static str> {
    use crate::task::{thread::ThreadState, waitqueue::WaitQueueHead};
    static POLL: WaitQueueHead = WaitQueueHead::new();
    let (s, n) = crate::time::timer::get_monotonic_time_ns();
    let wake = s
        .saturating_mul(1_000_000_000)
        .saturating_add(n)
        .saturating_add(10_000_000);
    // Prepared waits hold one PREEMPT level through publication and cleanup.
    // The recovery kthread has no syscall-entry bracket to supply that level.
    let unheld = crate::per_cpu::preempt_count() == 0;
    if unheld {
        crate::per_cpu::preempt_disable();
    }
    let outcome = POLL.prepare_to_wait_checked(ThreadState::BlockedOnIO, Some(wake), || true);
    let result = crate::syscall::blocking_io::wait_prepared(&POLL, outcome);
    if unheld {
        crate::per_cpu::preempt_enable();
    }
    result.map_err(|_| "native recovery worker wait failed")
}

pub(super) fn probe(s: &mut super::process::Session) -> Result<String, &'static str> {
    let mut regs = Registers(s.registers);
    let e = discover(&mut regs)?;
    let old = [regs.read(0x2254), regs.read(0x2140), regs.read(0x204)];
    regs.write(0x2140, 0);
    regs.write(e.base + 0x904, 0);
    regs.mask(0x204, 1 << e.pbdma, 1 << e.pbdma);
    regs.write(0x2254, 0x10000000 | (SLOT >> 12) as u32);
    let baseline = s.layout.allocator.stats();
    let status = system_dma::translation_status()?;
    let userd = s.layout.allocator.allocate(4096, 4096, Kind::Engine)?;
    let mut a = Channel::new(&mut regs, &mut s.layout, s.total, 0, &status, userd)?;
    let mut b = Channel::new(&mut regs, &mut s.layout, s.total, 1, &status, userd)?;
    let seed = timer::rdtsc() as u32;
    // B's reset workload is private, mapped before admission, and has no alias
    // to firmware/display storage. Eight transfers keep a real copy in flight.
    let active_source = s.layout.allocator.allocate(16 << 20, 4096, Kind::Engine)?;
    let active_dest = s.layout.allocator.allocate(16 << 20, 4096, Kind::Engine)?;
    for (va, allocation) in [(0x1000000, active_source), (0x2000000, active_dest)] {
        b.context.map(&mut s.layout.allocator, &mut VmIo {regs: &mut regs, total: s.total}, va, allocation, flags())?;
    }
    let nonce = seed | 1;
    words(&mut regs, s.total, active_source.range.start, &[nonce])?;
    words(&mut regs, s.total, active_dest.range.start, &[!nonce])?;
    for (i, c) in [&mut a, &mut b].into_iter().enumerate() {
        let data: Vec<_> = (0..SIZE)
            .map(|n| pattern(n, if i == 0 { seed } else { !seed }))
            .collect();
        fill(&mut regs, s.total, c.source.range.start, &data)?;
    }
    modeset::bar_flush(&mut regs)?;
    // Prepare private replacement storage before either identity is bound.
    // Its USERD is separate; publish it only after confirmed transport reset.
    let fresh_userd = s.layout.allocator.allocate(4096, 4096, Kind::Engine)?;
    let mut fresh_a = Channel::new(&mut regs, &mut s.layout, s.total, 0, &status, fresh_userd)?;
    let mut fresh_b = Channel::new(&mut regs, &mut s.layout, s.total, 1, &status, fresh_userd)?;
    for (i, c) in [&mut fresh_a, &mut fresh_b].into_iter().enumerate() {
        let data: Vec<_> = (0..SIZE)
            .map(|n| pattern(n, if i == 0 { seed } else { !seed }))
            .collect();
        fill(&mut regs, s.total, c.source.range.start, &data)?;
    }
    activate_pair(&mut regs, s.total, &e, s.bi, s.bd, s.pte, &mut a, &mut b)?;
    // B has executed before A faults, and remains bound throughout local recovery.
    let _ = compare(&mut regs, s.total, &e, s.bar, &mut b, !seed)?;
    let survivor_context = b.context.space().instance.range.start;
    let survivor_epoch = b.epoch;
    let invalid = SOURCE + SIZE as u64 + 4096;
    let held = a.context.hold(Consumer::Queued)?;
    let seq = a.submit(
        &mut regs,
        s.total,
        s.bar,
        rect(invalid, DEST, 1, 1, 1, 1),
        true,
    )?;
    let request = a.request(seq, a.dest.range.start, 1);
    let start = timer::rdtsc();
    while regs.read(0x259c) == 0 {
        if elapsed(start) >= 2_000_000 {
            return Err("bounded native invalid mapping did not fault");
        }
        pause()?;
    }
    let (mask, faults) = super::super::vm::faults(&mut VmIo {
        regs: &mut regs,
        total: s.total,
    });
    let f = faults
        .iter()
        .find(|f| u32::from(f.unit) == e.fault)
        .ok_or("native CE fault unit absent")?;
    if f.instance != a.context.space().instance.range.start
        || f.address != invalid
        || f.reason != 2
        || f.write
        || !a.context.attribute(f)
        || a.completed(&mut regs, s.total, &e, request, 0)?.0.is_some()
    {
        return Err("native fault attribution or completion differs");
    }
    let (instance, raw) = (f.instance, f.raw);
    quarantine(&mut regs, &mut a, "device-error");
    let fault_held = hold_check(&mut s.layout, &a)?;
    let refused = rejected(&mut regs, s.total, s.bar, &mut a)?;
    let (local_us, local_disabled, local_enabled) =
        fault_reset(&mut regs, s.total, &e, &mut a, &b, mask)?;
    unsafe { a.context.confirm_reset(); }
    a.active = false;
    drop(held);
    let binding = regs.read(0x800000);
    if regs.read(0x800008) != 0x80000000 | (survivor_context >> 12) as u32 {
        return Err("local fault replaced surviving binding");
    }
    let survivor = compare(&mut regs, s.total, &e, s.bar, &mut b, !seed)?;
    let survivor_after_context = u64::from(regs.read(0x800008) & 0x0fffffff) << 12;
    let survivor_after_epoch = u64::from(read(&mut regs, s.total, b.release_address(b.put) + 4)?)
        | u64::from(read(&mut regs, s.total, b.release_address(b.put) + 8)?) << 32;
    let local_pending = regs.read(0x2284 + e.run as usize * 8) & 0x100000;
    let local_busy = regs.read(0x2640 + e.engine as usize * 8) & 0xc0000000;
    let local_errors = regs.read(0x259c) | regs.read(e.base + 0x908) & 7;
    let fault=format!("{{\"instance\":{},\"invalid\":{},\"raw\":{:?},\"mask\":{},\"held\":{},\"rejected\":{},\"removed\":{},\"runlist\":[1],\"binding\":{},\"survivor_context\":{},\"survivor_epoch\":{},\"survivor_after_context\":{},\"survivor_after_epoch\":{},\"survivor_digests\":{},\"reset_us\":{},\"reset_bit\":{},\"reset_disabled\":{},\"reset_enabled\":{},\"pending\":{},\"busy\":{},\"errors\":{},\"confirmed\":true}}",instance,invalid,raw,mask,fault_held,refused,binding==0,binding,survivor_context,survivor_epoch,survivor_after_context,survivor_after_epoch,survivor,local_us,e.reset,local_disabled,local_enabled,local_pending,local_busy,local_errors);
    a.retire(&mut regs, &mut s.layout, s.total)?;
    b.drain(&mut regs, &e)?;
    let mut a = Channel::new(&mut regs, &mut s.layout, s.total, 0, &status, userd)?;
    let data: Vec<_> = (0..SIZE).map(|n| pattern(n, seed)).collect();
    fill(&mut regs, s.total, a.source.range.start, &data)?;
    modeset::bar_flush(&mut regs)?;
    activate_pair(&mut regs, s.total, &e, s.bi, s.bd, s.pte, &mut a, &mut b)?;
    let (held, request) = gate(&mut regs, s.total, s.bar, &mut a)?;
    let start = timer::rdtsc();
    let mut watchdog = Watchdog::new(0);
    let quarantine_us = loop {
        if a.completed(&mut regs, s.total, &e, request, 0)?.0.is_some() {
            return Err("withheld native work completed");
        }
        let now = elapsed(start);
        match watchdog.scan(now) {
            Action::Quarantine => break now,
            Action::None => pause()?,
            Action::Reset => return Err("reset before quarantine"),
        }
    };
    quarantine(&mut regs, &mut a, "device-hung");
    let held_at_quarantine = hold_check(&mut s.layout, &a)?;
    let refused = rejected(&mut regs, s.total, s.bar, &mut a)?;
    if remove(&mut regs, s.total, &e, &a, &b).is_err() {
        // Preempt/runlist timeout is not a terminal parked-runlist state.
        // Escalate to CE reset, with the same survivor and retained backing.
        fault_reset(&mut regs, s.total, &e, &mut a, &b, 0)?;
    }
    a.active = false;
    let binding = regs.read(0x800000);
    let survivor = compare(&mut regs, s.total, &e, s.bar, &mut b, !seed)?;
    let progress = elapsed(start);
    watchdog.progress(progress);
    if watchdog.scan(progress) != Action::None {
        return Err("survivor progress triggered reset");
    }
    // Keep a second pending request on the other context while reset fails
    // its outstanding fence. Its acquire stays withheld until reset.
    let held_b = b.context.hold(Consumer::Queued)?;
    let seq = b.queue.next()?;
    let rectangle = rect(0x1000000, 0x2000000, 8192, 2048, 8192, 8192);
    let mut commands = vec![(1 << 29) | (4 << 16) | 4, 0, (COMMAND + 9216) as u32, 1, 0x1001];
    for _ in 0..7 { commands.extend(copy::transfer(rectangle)?); }
    commands.extend(copy::encode(rectangle, COMMAND + 8192 + u64::from(super::super::queue::Queue::slot(seq)) * 16, b.epoch, seq)?);
    b.push(&mut regs, s.total, s.bar, &commands, true)?;
    let request_b = b.request(seq, active_dest.range.start, 1);
    loop {
        if watchdog.scan(elapsed(start)) == Action::Reset {
            break;
        }
        pause()?;
    }
    let decision = elapsed(start);
    // Start the queued copies only once the no-progress decision is made.
    // Require observed DMA progress AND an absent final release before reset.
    words(&mut regs, s.total, b.commands.range.start + 9216, &[1])?;
    modeset::bar_flush(&mut regs)?;
    let active_start = timer::rdtsc();
    let (active_busy, active_word) = loop {
        let busy = regs.read(0x2640 + e.engine as usize * 8);
        let word = read(&mut regs, s.total, active_dest.range.start)?;
        if b.completed(&mut regs, s.total, &e, request_b, 0)?.0.is_some() {
            return Err("active reset copy finished before observation");
        }
        if busy & 0x80000000 != 0 && word == nonce { break (busy, word); }
        if elapsed(active_start) >= 100_000 { return Err("active reset copy never started"); }
    };
    let reset_start = timer::rdtsc();
    let old_epochs = [a.epoch, b.epoch];
    let mut hardware_fences = [[0u32; 3]; 2];
    for (i, (c, request)) in [(&a, request), (&b, request_b)].into_iter().enumerate() {
        for (j, value) in hardware_fences[i].iter_mut().enumerate() {
            *value = read(&mut regs, s.total, c.release_address(request.sequence) + j as u64 * 4)?;
        }
    }
    let expected_fences = [
        [request.sequence, a.epoch as u32, (a.epoch >> 32) as u32],
        [request_b.sequence, b.epoch as u32, (b.epoch >> 32) as u32],
    ];
    if hardware_fences.iter().zip(expected_fences).any(|(actual, expected)| *actual == expected) {
        return Err("old hardware fence released before reset");
    }
    let hardware_completed = [
        a.hardware_completed(&mut regs, s.total, &e, request, 0)?.0.is_some(),
        b.hardware_completed(&mut regs, s.total, &e, request_b, 0)?.0.is_some(),
    ];
    if hardware_completed.iter().any(|done| *done) {
        return Err("old request completed before transport reset");
    }
    quarantine(&mut regs, &mut a, "device-reset");
    quarantine(&mut regs, &mut b, "device-reset");
    let before = [
        hold_check(&mut s.layout, &a)?,
        hold_check(&mut s.layout, &b)?,
    ];
    // The engine is still enabled and B still bound: the actual reset receipt
    // validator must refuse these real pre-reset observations, retaining holds.
    let not_disabled = regs.read(0x200);
    let failed_reset_observation = [not_disabled, regs.read(0x800000), regs.read(0x800008)];
    let failed_reset_rejected = matches!(
        transport_confirmed(&mut regs, &e, not_disabled, not_disabled),
        Err("transport reset unconfirmed; backing retained")
    );
    if !failed_reset_rejected {
        return Err("unperformed reset produced a trusted receipt");
    }
    let failed_held = [
        hold_check(&mut s.layout, &a)?,
        hold_check(&mut s.layout, &b)?,
    ];
    let old_eio = [
        matches!(
            a.completed(&mut regs, s.total, &e, request, 0),
            Err("device-reset")
        ),
        matches!(
            b.completed(&mut regs, s.total, &e, request_b, 0),
            Err("device-reset")
        ),
    ];
    regs.mask(0x2630, 1 << e.run, 1 << e.run);
    // Finish preempting the resident channel before resetting its engine.
    // A request alone can still overlap the reset with the old context save.
    regs.write(0x2634, b.id);
    budget_wait(&mut regs, reset_start, 0x2634, 0x100000)?;
    regs.mask(0x200, 1 << e.reset, 0);
    let disabled = regs.read(0x200);
    regs.delay_us(50);
    regs.mask(0x200, 1 << e.reset, 1 << e.reset);
    let enabled = regs.read(0x200);
    regs.write(e.base + 0x908, 7);
    regs.write(e.base + 0xf14, 0);
    regs.write(0x2270, ((a.commands.range.start + RUNLIST) >> 12) as u32);
    regs.write(0x2274, e.run << 20);
    regs.write(0x262c, 1 << e.run);
    regs.mask(0x2630, 1 << e.run, 0);
    budget_wait(
        &mut regs,
        reset_start,
        0x2284 + e.run as usize * 8,
        0x100000,
    )?;
    regs.write(0x2634, 1);
    budget_wait(&mut regs, reset_start, 0x2634, 0x100000)?;
    budget_wait(&mut regs, reset_start, 0x3080 + e.pbdma * 4, 0xe000)?;
    budget_wait(
        &mut regs,
        reset_start,
        0x2640 + e.engine as usize * 8,
        0xc0000000,
    )?;
    regs.write(0x800000, 0);
    regs.write(0x800008, 0);
    transport_confirmed(&mut regs, &e, disabled, enabled)?;
    let reset_pending = regs.read(0x2284 + e.run as usize * 8) & 0x100000;
    let reset_busy = regs.read(0x2640 + e.engine as usize * 8) & 0xc0000000;
    let reset_errors = regs.read(0x259c) | regs.read(e.base + 0x908) & 7;
    unsafe {
        a.context.confirm_reset();
        b.context.confirm_reset();
    }
    a.active = false;
    b.active = false;
    drop(held);
    drop(held_b);
    activate_pair(
        &mut regs,
        s.total,
        &e,
        s.bi,
        s.bd,
        s.pte,
        &mut fresh_a,
        &mut fresh_b,
    )?;
    let reset_us = elapsed(reset_start);
    if reset_us > 100_000 {
        return Err("native transport reinitialization exceeds reset budget");
    }
    let bindings = [regs.read(0x800000), regs.read(0x800008)];
    let fresh_contexts = [
        fresh_a.context.space().instance.range.start,
        fresh_b.context.space().instance.range.start,
    ];
    if bindings != fresh_contexts.map(|c| 0x80000000 | (c >> 12) as u32) {
        return Err("reset channel publication unconfirmed");
    }
    let new_epochs = [fresh_a.epoch, fresh_b.epoch];
    let _ = compare(&mut regs, s.total, &e, s.bar, &mut fresh_a, seed)?;
    let stale = fresh_a.request(fresh_a.put, fresh_a.dest.range.start, 1);
    words(
        &mut regs,
        s.total,
        fresh_a.release_address(fresh_a.put) + 4,
        &[old_epochs[0] as u32, (old_epochs[0] >> 32) as u32],
    )?;
    modeset::bar_flush(&mut regs)?;
    let stale_accepted = fresh_a
        .completed(&mut regs, s.total, &e, stale, 0)?
        .0
        .is_some();
    if stale_accepted {
        return Err("old reset epoch accepted");
    }
    let fresh = [
        compare(&mut regs, s.total, &e, s.bar, &mut fresh_a, seed)?,
        compare(&mut regs, s.total, &e, s.bar, &mut fresh_b, !seed)?,
    ];
    let active_record = format!("{{\"busy\":{},\"word\":{},\"nonce\":{},\"held\":{},\"reset_us\":{},\"reset_disabled\":{},\"reset_enabled\":{},\"reset_bit\":{},\"fresh_digests\":{}}}", active_busy, active_word, nonce, before[1], reset_us, disabled, enabled, e.reset, fresh[1]);
    let total_us = elapsed(start);
    let watchdog=format!("{{\"quarantine_us\":{},\"no_progress_us\":{},\"total_us\":{},\"held_at_quarantine\":{},\"held_before_reset\":{:?},\"failed_reset_held\":{:?},\"failed_reset_rejected\":{},\"failed_reset_observation\":{:?},\"rejected\":{},\"removed\":{},\"runlist\":[1],\"binding\":{},\"success_before_reset\":{},\"hardware_completed\":{:?},\"hardware_fences\":{:?},\"expected_fences\":{:?},\"survivor_digests\":{},\"reset_us\":{},\"reset_bit\":{},\"reset_disabled\":{},\"reset_enabled\":{},\"pending\":{},\"busy\":{},\"errors\":{},\"confirmed\":true,\"old_epochs\":{:?},\"new_epochs\":{:?},\"old_fences_eio\":{:?},\"stale_accepted\":{},\"bindings\":{:?},\"fresh_digests\":[{},{}]}}",quarantine_us,decision-progress,total_us,held_at_quarantine,before,failed_held,failed_reset_rejected,failed_reset_observation,refused,binding==0,binding,hardware_completed.iter().any(|v|*v),hardware_completed,hardware_fences,expected_fences,survivor,reset_us,e.reset,disabled,enabled,reset_pending,reset_busy,reset_errors,old_epochs,new_epochs,old_eio,stale_accepted,bindings,fresh[0],fresh[1]);
    fresh_a.drain(&mut regs, &e)?;
    fresh_b.drain(&mut regs, &e)?;
    words(&mut regs, s.total, s.pte, &[0, 0])?;
    modeset::activate_bar1(&mut regs, s.bi, s.bd)?;
    for c in [a, b, fresh_a, fresh_b] {
        c.retire(&mut regs, &mut s.layout, s.total)?;
    }
    s.layout.allocator.release(active_source)?;
    s.layout.allocator.release(active_dest)?;
    s.layout.allocator.release(userd)?;
    s.layout.allocator.release(fresh_userd)?;
    regs.write(0x2254, old[0]);
    regs.write(0x2140, old[1]);
    regs.write(0x204, old[2]);
    if s.layout.allocator.stats() != baseline {
        return Err("native recovery baseline differs");
    }
    Ok(format!("GT710_RECOVERY {{\"schema\":1,\"seed\":{},\"worker\":true,\"poisons\":[85,170],\"baseline\":{},\"after\":{},\"firmware_writes\":{},\"fault\":{},\"watchdog\":{},\"active_reset\":{}}}\n",seed,stats(baseline),stats(s.layout.allocator.stats()),crate::firmware_writes::count(),fault,watchdog,active_record))
}

/// Shared CE reset pulse and read-back, also used for the GR-runlist CE2.
/// Channel backing reclamation still requires the complete FIFO unwind.
pub(in crate::gk208) fn reset_engine(
    regs: &mut Registers, base: usize, reset: u32,
) -> Result<(u32, u32), &'static str> {
    regs.mask(0x200, 1 << reset, 0);
    let disabled = regs.read(0x200);
    regs.delay_us(50);
    regs.mask(0x200, 1 << reset, 1 << reset);
    let enabled = regs.read(0x200);
    if disabled & (1 << reset) != 0 || enabled & (1 << reset) == 0 {
        return Err("CE reset unconfirmed; backing retained");
    }
    regs.write(base + 0x908, 7);
    regs.write(base + 0xf14, 0);
    if regs.read(base + 0x908) & 7 != 0 { return Err("CE reset fault; backing retained"); }
    Ok((disabled, enabled))
}
