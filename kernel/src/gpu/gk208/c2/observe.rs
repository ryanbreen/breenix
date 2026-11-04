/*
 * Copyright 2012 Red Hat Inc.
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
 * Copyright (C) 2011 Martin Peres <martin.peres@ensi-bourges.fr>
 * Copyright (C) 2011 Marcelina Kościelnicka <mwk@0x04.net>
 * All Rights Reserved.
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sublicense,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice (including the next
 * paragraph) shall be included in all copies or substantial portions of the
 * Software.
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
 * Copyright 2006 Artur Huillet, Ben Skeggs, B. R., Carlos Martin, Christoph Bumiller, Dawid Gajownik, Dmitry Baryshkov, Dmitry Eremin-Solenikov, EdB, Erik Waling, Francisco Jerez, Ilia Mirkin, jb17bsome, Jeremy Kolb, Laurent Carlier, Luca Barbieri, Maarten Maathuis, Marcelina Kościelnicka, Mark Carey, Matthieu Castet, nvidiaman, Patrice Mandin, Pekka Paalanen, Peter Popov, Richard Hughes, Rudi Cilibrasi, Serge Martin, Simon Raffeiner, Stephane Loeuillet, Stephane Marchesin, sturmflut, Sylvain Munaut, Victor Stinner, Wladmir van der Laan, Younes Manton.
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
//! C2d: inactive-path snapshots, exact POST restore, and GPC counter/PTIMER.
//! MIT sources and exact revisions are listed in docs/gui/gt710-c2.md.
use super::Access;
use alloc::vec::Vec;

pub fn capture(io: &mut impl Access) -> Vec<(u32, u32)> {
    let mut addresses = alloc::vec![
        0x101000, 0xe800, 0xe804, 0xe808, 0xe820, 0xe824, 0xe828,
        0x132000, 0x132004, 0x132008, 0x132020, 0x132024, 0x132028, 0x132030,
        0x137100, 0x137320, 0x137330, 0x1373f4, 0x9200, 0x9210, 0x9220,
    ];
    // Read inactive paths too. gk104.c read/calc/prog covers the four PLLs,
    // eight reference slots and sixteen engine slots, including shared CLK7.
    for i in [0, 1, 2, 7] {
        for base in [0x137000, 0x137004, 0x137008] { addresses.push(base + i * 0x20); }
    }
    for i in 0..8 {
        for base in [0x137120, 0x137140] { addresses.push(base + i * 4); }
    }
    for i in 0..16 {
        for base in [0x137160, 0x1371d0, 0x137250] { addresses.push(base + i * 4); }
    }
    addresses.sort_unstable();
    addresses.dedup();
    addresses.into_iter().map(|a| (a, io.read(a))).collect()
}

fn value(snapshot: &[(u32, u32)], address: u32) -> Result<u32, &'static str> {
    let mut words = snapshot.iter().filter(|p| p.0 == address);
    let word = words.next().ok_or("incomplete POST snapshot")?.1;
    if words.next().is_some() { return Err("duplicate snapshot word"); }
    Ok(word)
}

#[cfg(any(feature="gt710_clock_probe", test))]
pub fn differences(post: &[(u32, u32)], now: &[(u32, u32)])
    -> Result<Vec<(u32, u32, u32)>, &'static str> {
    if post.len() != now.len() { return Err("snapshot geometry changed"); }
    let mut result = Vec::new();
    for &(a, before) in post {
        let after = value(now, a)?;
        if before != after { result.push((a, before, after)); }
    }
    Ok(result)
}

pub fn restore(io: &mut impl Access, post: &[(u32, u32)]) -> Result<(), &'static str> {
    // Validate before the first write; only these five registers C2 can change.
    let select = value(post, 0x137100)?;
    let ctrl = value(post, 0x137000)?;
    let coef = value(post, 0x137004)?;
    let source = value(post, 0x137160)?;
    let divider = value(post, 0x1371d0)?;
    if select & 1 != 0 && ctrl & 1 == 0 { return Err("POST selects disabled PLL"); }
    if io.read(0x137100) & !1 != select & !1 { return Err("non-GPC source changed"); }
    // gk104.c prog stages 1,2,4: off PLL before disabling/programming it.
    io.write(0x137100, select & !1);
    io.wait(0x137100, 1, 0)?;
    let current = io.read(0x137000);
    io.write(0x137000, current & !4);
    io.write(0x137000, current & !5);
    io.write(0x137004, coef);
    io.write(0x137000, ctrl & !0x15);
    if ctrl & 1 != 0 {
        io.write(0x137000, (ctrl & !0x15) | 1);
        io.wait(0x137000, 0x20000, 0x20000)?;
    }
    io.write(0x137000, ctrl);
    if select & 1 != 0 {
        io.write(0x137100, select);
        io.wait(0x137100, 1, 1)?;
        // Restore the unused alternate path only after leaving it.
        io.write(0x1371d0, divider);
        io.write(0x137160, source);
    } else {
        io.write(0x1371d0, divider);
        io.write(0x137160, source);
        io.write(0x137100, select);
        io.wait(0x137100, 1, 0)?;
    }
    for a in [0x137100, 0x137000, 0x137004, 0x137160, 0x1371d0] {
        if io.read(a) != value(post, a)? { return Err("POST register restore mismatch"); }
    }
    Ok(())
}

pub trait CounterAccess: Access {
    fn monotonic_ns(&mut self) -> u64;
    fn record(&mut self, stage: &str, raw: &[(u32, u32)]);
}
/// The five sampler registers, including both PTIMER time words.
pub fn counter_snapshot(io: &mut impl Access) -> [(u32, u32); 5] {
    [0x18009c, 0x180044, 0x1800a8, 0x9400, 0x9410].map(|a| (a, io.read(a)))
}
pub struct Measurement {
    pub khz: u32,
    pub cycles: u32,
    pub ptimer_ns: u64,
    pub bracket_ns: u64,
}
fn ptimer(io: &mut impl Access) -> Result<u64, &'static str> {
    // nv04_timer_read + rnndb bus/ptimer.xml: high/low/high, nanoseconds.
    for _ in 0..4 {
        let high = io.read(0x9410);
        let low = io.read(0x9400);
        if high == io.read(0x9410) { return Ok((u64::from(high) << 32) | u64::from(low)); }
    }
    Err("PTIMER unstable")
}
fn point(io: &mut impl Access) -> Result<(u64, u32, u64), &'static str> {
    let a = ptimer(io)?;
    let cycles = io.read(0x1800a8);
    let b = ptimer(io)?;
    let span = b.checked_sub(a).ok_or("PTIMER went backwards")?;
    Ok((a + span / 2, cycles, span))
}
pub fn measure(io: &mut impl CounterAccess) -> Result<Measurement, &'static str> {
    // MIT nouveau pm/gf100.c:175-180 identifies the PM master bit.
    // mc/nv04.c:40-45 enables a masked unit, followed by a posted read.
    // GK208 mc/gk20a.c uses that device callback; no separate PM clock-gating
    // register is touched by this PM/MC path. Preserve each unrelated unit.
    let pmc = io.read(0x200);
    io.record("pm-before", &[(0x200, pmc)]);
    let before = counter_snapshot(io);
    io.record("before-pm-enable", &before);
    io.write(0x200, pmc | 0x10000000);
    let enabled = io.read(0x200);
    io.record("pm-enabled", &[(0x200, enabled)]);
    let after = counter_snapshot(io);
    io.record("after-pm-enable", &after);
    let result = if enabled != pmc | 0x10000000 {
        Err("PM enable readback mismatch")
    } else if after[..3].iter().any(|p| p.1 & 0xffff0000 == 0xbadf0000) {
        // These are PRI error words, not a saved owner/mode/accumulator.
        // Do not attempt to restore them by writing them into the counter.
        Err("GPC counter has no PRI response after PM enable")
    } else {
        measure_enabled(io)
    };
    // Counter cleanup completes inside measure_enabled before PM is restored.
    io.write(0x200, pmc);
    let restored = io.read(0x200);
    io.record("pm-restored", &[(0x200, restored)]);
    let counters = counter_snapshot(io);
    io.record("after-pm-restore", &counters);
    if restored != pmc { return Err("PM restore readback mismatch"); }
    result
}

fn measure_enabled(io: &mut impl CounterAccess) -> Result<Measurement, &'static str> {
    // envytools nvatiming.c:171-242, GPC[0] PRE with constant-true logic.
    // rnndb gf100_pcounter.xml: PRE_OP, CTRL and CTR_PRE_NEXT at 0x180000.
    // No signal routing, clocks or PTIMER configuration is written.
    let saved = counter_snapshot(io);
    io.record("saved", &saved);
    let [(_, ctrl), (_, pre), (_, count), _, _] = saved;
    // C2c: inherited MODE is power-on state, not evidence of another owner.
    // Halt it before replacing PRE_OP; restore each written word on either path.
    io.write(0x18009c, 0);
    io.write(0x180044, 0xffff);
    io.write(0x18009c, 0x40002);
    let configured = counter_snapshot(io);
    io.record("configured", &configured);
    let result = (|| {
        if configured[..3].iter().any(|p| p.1 & 0xffff0000 == 0xbadf0000) {
            return Err("GPC counter lost PRI response during setup");
        }
        let start = io.monotonic_ns();
        for attempt in 0..3 {
            let (t0, c0, b0) = point(io)?;
            loop {
                if io.monotonic_ns().saturating_sub(start) >= 100_000_000 {
                    return Err("GPC counter/PTIMER timeout");
                }
                let (t1, c1, b1) = point(io)?;
                let elapsed = t1.checked_sub(t0).ok_or("PTIMER went backwards")?;
                if elapsed < 20_000_000 { continue; }
                let cycles = c1.wrapping_sub(c0);
                if cycles == 0 { return Err("GPC counter did not advance"); }
                let bracket_ns = b0 + b1;
                if bracket_ns > elapsed / 100 {
                    if attempt == 2 { return Err("counter/PTIMER bracket exceeds 1 percent"); }
                    break;
                }
                let khz = u32::try_from(u64::from(cycles) * 1_000_000 / elapsed)
                    .map_err(|_| "measured clock overflow")?;
                return Ok(Measurement { khz, cycles, ptimer_ns: elapsed, bracket_ns });
            }
        }
        Err("counter/PTIMER bracket exceeds 1 percent")
    })();
    // Cleanup follows sampling success or failure, including a stopped PTIMER/counter.
    io.write(0x18009c, 0);
    io.write(0x1800a8, count);
    io.write(0x180044, pre);
    // Verify the saved accumulator while halted: resuming an inherited active
    // counter can immediately advance it. The resumed raw value is logged too.
    let halted = counter_snapshot(io);
    io.record("restore-halted", &halted);
    let halted_ok = halted[..3] == [(0x18009c, 0), (0x180044, pre), (0x1800a8, count)];
    io.write(0x18009c, ctrl);
    let restored = counter_snapshot(io);
    io.record("restored", &restored);
    if !halted_ok || restored[0].1 != ctrl || restored[1].1 != pre
        || (ctrl & 7 == 0 && restored[2].1 != count) {
        return Err("GPC counter restore mismatch");
    }
    result
}
