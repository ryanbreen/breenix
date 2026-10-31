/*
 * Copyright 2012 Red Hat Inc.
 * Copyright 2014 Martin Peres
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
 * 	    Martin Peres
 */
//! Restore-only C1 sensor read: MIT therm/g84.c:29-52, fuse/gf100.c:27-45.
//! K1 permits this bounded setup without alarm/fan/clock programming.
pub trait Io {
    fn read(&mut self, address: u32) -> u32;
    fn write(&mut self, address: u32, value: u32);
    fn stabilize(&mut self);
}
pub struct Reading { pub value: Result<u32, &'static str>, pub restored: bool }
pub fn temperature(io: &mut impl Io) -> Reading {
    let enable = io.read(0x22400);
    let access = io.read(0x21000);
    if enable & 0x800 == 0 { io.write(0x22400, enable | 0x800); }
    if access & 1 == 0 { io.write(0x21000, access | 1); }
    let present = io.read(0x212a8) == 1;
    if access & 1 == 0 { io.write(0x21000, access); }
    if enable & 0x800 == 0 { io.write(0x22400, enable); }
    let mut restored = io.read(0x21000) == access && io.read(0x22400) == enable;
    let value = if present {
        let a = io.read(0x20008); let b = io.read(0x2000c);
        let setup = a & 0x80008000 != 0x80000000 || b & 0x80000003 != 0;
        if setup {
            io.write(0x20008, (a & !0x80008000) | 0x80000000);
            io.write(0x2000c, b & !0x80000003);
            io.stabilize();
        }
        let temperature = io.read(0x20400);
        if setup { io.write(0x2000c,b); io.write(0x20008,a); }
        restored &= io.read(0x20008) == a && io.read(0x2000c) == b;
        if temperature > i32::MAX as u32 { Err("sensor-read-error") } else { Ok(temperature) }
    } else { Err("sensor-unavailable") };
    Reading { value: if restored { value } else { Err("sensor-restore-failed") }, restored }
}
