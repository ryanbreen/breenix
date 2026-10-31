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
//! Read-only C1 sensor observation: MIT therm/g84.c:29-52, fuse/gf100.c:27-45.
//! C1 r3 defers setup to a later step; the callback cannot write registers.
pub struct Reading { pub value: Result<u32, &'static str>, pub restored: bool }
pub fn temperature(mut read: impl FnMut(u32) -> u32) -> Reading {
    let enable = read(0x22400);
    let access = read(0x21000);
    let value = if enable & 0x800 == 0 || access & 1 == 0 {
        Err("needs-setup")
    } else if read(0x212a8) != 1 {
        Err("sensor-unavailable")
    } else {
        let a = read(0x20008);
        let b = read(0x2000c);
        if a & 0x80008000 != 0x80000000 || b & 0x80000003 != 0 {
            Err("needs-setup")
        } else {
            let temperature = read(0x20400);
            if temperature > i32::MAX as u32 { Err("sensor-read-error") } else { Ok(temperature) }
        }
    };
    // Retain the schema-1 field for the read-only observation.
    Reading { value, restored: true }
}
