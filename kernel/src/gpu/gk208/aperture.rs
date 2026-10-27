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
//! Bounded CPU BAR controls; no channels are admitted here.
use super::modeset;
pub trait Io: modeset::Io {
    fn drain_wc(&mut self);
}
/// Drains CPU WC first, publishes a private instance, then invalidates its PDB.
pub fn activate(
    a: &mut impl Io,
    bar: u8,
    instance: u64,
    directory: u64,
) -> Result<(), &'static str> {
    if !matches!(bar, 1 | 2)
        || instance == 0
        || directory == 0
        || instance & 4095 != 0
        || directory & 4095 != 0
        || instance >= 1 << 40
        || directory >= 1 << 40
    {
        return Err("invalid CPU BAR binding");
    }
    a.drain_wc();
    modeset::bar_flush(a)?;
    a.write(
        if bar == 1 { 0x1704 } else { 0x1714 },
        0x80000000 | (instance >> 12) as u32,
    );
    modeset::bar1_wait(a)?;
    modeset::bar_flush(a)?;
    let mut free = false;
    for _ in 0..20000 {
        if a.read(0x100c80) & 0x00ff0000 != 0 {
            free = true;
            break;
        }
        a.delay_us(100);
    }
    if !free {
        return Err("CPU BAR invalidation queue full");
    }
    a.write(0x100cb8, (directory >> 8) as u32);
    a.write(0x100cbc, 0x80000005);
    modeset::wait(a, 0x100c80, 0x8000, 0x8000)
}
