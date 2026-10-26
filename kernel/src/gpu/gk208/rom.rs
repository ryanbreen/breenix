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
 */
/*
 * Copyright 2015 Red Hat Inc.
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
//! Direct PROM access for a POSTed GK208B, independent of inherited VRAM windows.
extern crate alloc;
use alloc::{vec, vec::Vec};

pub trait Access {
    fn read(&self, reg: usize) -> u32;
    fn write(&mut self, reg: usize, value: u32);
}

pub fn read(regs: &mut impl Access) -> Result<Vec<u8>, &'static str> {
    // Nouveau shadowrom.c and pci/{base,nv40}.c (MIT): bit 0 selects
    // shadow ROM; clear it for the PROM aperture, then restore the exact value.
    let saved = regs.read(0x88050);
    regs.write(0x88050, saved & !1);
    let result = (|| {
        let mut data = vec![0u8; 512];
        for p in (0..512).step_by(4) {
            data[p..p + 4].copy_from_slice(&regs.read(0x300000 + p).to_le_bytes());
        }
        if data[..2] != [0x55, 0xaa] {
            return Err("PROM signature");
        }
        let pcir = u16::from_le_bytes([data[0x18], data[0x19]]) as usize;
        if pcir + 0x16 > 512 || data[pcir..pcir + 4] != *b"PCIR" {
            return Err("PROM PCIR");
        }
        let len = u16::from_le_bytes([data[pcir + 16], data[pcir + 17]]) as usize * 512;
        if !(512..=256 * 1024).contains(&len) {
            return Err("PROM length bound");
        }
        data.resize(len, 0);
        for p in (512..len).step_by(4) {
            data[p..p + 4].copy_from_slice(&regs.read(0x300000 + p).to_le_bytes());
        }
        Ok(data)
    })();
    regs.write(0x88050, saved);
    result
}
