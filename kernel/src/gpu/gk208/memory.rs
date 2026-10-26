/*
 * Copyright 2017 Red Hat Inc.
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
 * Copyright 2017 Red Hat Inc.
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
//! E19 PR 4.2: bounded POST-era display memory, not a per-context VM allocator.
//! Hardware fields adapted from the individually checked MIT nouveau v6.8 files
//! listed in THIRD_PARTY_NOTICES. Called on the boot CPU before GPU clients exist.
use alloc::{vec, vec::Vec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: u64,
    pub size: u64,
}
impl Range {
    pub fn end(self) -> Option<u64> {
        self.start.checked_add(self.size)
    }
    pub fn overlaps(self, other: Self) -> bool {
        self.start < other.end().unwrap_or(u64::MAX) && other.start < self.end().unwrap_or(u64::MAX)
    }
}
#[derive(Debug)]
pub struct Object {
    pub name: &'static str,
    pub range: Range,
    pub align: u64,
}
impl Object {
    pub fn guarded(&self) -> Range {
        Range {
            start: self.range.start - 4096,
            size: self.range.size + 8192,
        }
    }
}
#[derive(Debug)]
pub struct Layout {
    pub pitch: u64,
    pub objects: Vec<Object>,
}
impl Layout {
    pub fn new(
        total: u64,
        reserved: &[Range],
        width: u32,
        height: u32,
    ) -> Result<Self, &'static str> {
        if total == 0
            || total > 1 << 32
            || width == 0
            || height == 0
            || width > 8192
            || height > 8192
        {
            return Err("unsupported display memory geometry");
        }
        if reserved
            .iter()
            .any(|r| r.size == 0 || r.end().is_none_or(|e| e > total))
        {
            return Err("invalid inherited reservation");
        }
        let pitch = (u64::from(width) * 4 + 255) & !255;
        let size = (pitch * u64::from(height) + 4095) & !4095;
        let mut objects: Vec<Object> = Vec::new();
        let mut cursor = 4096;
        for (name, bytes, align) in [
            ("scanout-a", size, 65536),
            ("scanout-b", size, 65536),
            ("evo", 4096, 4096),
            ("notifier", 4096, 4096),
            ("instance", 65536, 65536),
            ("dma", 4096, 4096),
        ] {
            loop {
                let start = (cursor + align - 1) & !(align - 1);
                let object = Object {
                    name,
                    range: Range { start, size: bytes },
                    align,
                };
                let guard = object.guarded();
                if guard.end().is_none_or(|e| e > total) {
                    return Err("display carveout exhausted");
                }
                let bump = reserved
                    .iter()
                    .copied()
                    .chain(objects.iter().map(Object::guarded))
                    .filter(|r| r.overlaps(guard))
                    .filter_map(Range::end)
                    .max();
                if let Some(end) = bump {
                    cursor = end + 4096;
                } else {
                    cursor = object.guarded().end().ok_or("object overflow")? + 4096;
                    objects.push(object);
                    break;
                }
            }
        }
        Ok(Self { pitch, objects })
    }
}
pub trait Access {
    fn read(&mut self, reg: usize) -> u32;
    fn write(&mut self, reg: usize, value: u32);
}
pub struct Pramin<'a, A: Access> {
    access: &'a mut A,
    total: u64,
    saved: u32,
    bank: Option<u32>,
}
impl<'a, A: Access> Pramin<'a, A> {
    pub fn new(access: &'a mut A, total: u64) -> Self {
        let saved = access.read(0x1700);
        Self {
            access,
            total,
            saved,
            bank: None,
        }
    }
    fn window(&mut self, address: u64) -> Result<usize, &'static str> {
        if address & 3 != 0 || address.checked_add(4).is_none_or(|e| e > self.total) {
            return Err("PRAMIN address outside VRAM");
        }
        let bank = ((address & !0xfffff) >> 16) as u32;
        if self.bank != Some(bank) {
            self.access.write(0x1700, bank);
            if self.access.read(0x1700) != bank {
                return Err("PRAMIN window did not latch");
            }
            self.bank = Some(bank);
        }
        Ok(0x700000 + (address & 0xfffff) as usize)
    }
    pub fn read(&mut self, address: u64) -> Result<u32, &'static str> {
        let r = self.window(address)?;
        Ok(self.access.read(r))
    }
    pub fn write(&mut self, address: u64, value: u32) -> Result<(), &'static str> {
        let r = self.window(address)?;
        self.access.write(r, value);
        Ok(())
    }
    fn read64(&mut self, address: u64) -> Result<u64, &'static str> {
        Ok(u64::from(self.read(address)?) | (u64::from(self.read(address + 4)?) << 32))
    }
    pub fn bytes(&mut self, range: Range) -> Result<Vec<u8>, &'static str> {
        if range.size > 1 << 20 || range.size & 3 != 0 {
            return Err("snapshot exceeds bound");
        }
        let mut bytes = Vec::with_capacity(range.size as usize);
        for a in (range.start..range.end().ok_or("snapshot overflow")?).step_by(4) {
            bytes.extend_from_slice(&self.read(a)?.to_le_bytes());
        }
        Ok(bytes)
    }
}
impl<A: Access> Drop for Pramin<'_, A> {
    fn drop(&mut self) {
        self.access.write(0x1700, self.saved);
        let _ = self.access.read(0x1700);
    }
}

pub struct Inventory {
    pub total: u64,
    pub reserved: Vec<Range>,
    pub references: Vec<Range>,
    pub registers: Vec<(usize, u32)>,
}
fn reference(refs: &mut Vec<Range>, total: u64, start: u64, size: u64) -> Result<(), &'static str> {
    let r = Range { start, size };
    if start == 0 || start & 4095 != 0 || r.end().is_none_or(|e| e > total) {
        return Err("inherited instance outside VRAM");
    }
    if !refs.contains(&r) {
        refs.push(r);
    }
    Ok(())
}
pub fn discover<A: Access>(a: &mut A, aperture: u64) -> Result<Inventory, &'static str> {
    let mut registers = Vec::new();
    let fbps = a.read(0x22438);
    let disabled = a.read(0x22554);
    registers.extend([(0x22438, fbps), (0x22554, disabled)]);
    if !(1..=8).contains(&fbps) {
        return Err("invalid FBP count");
    }
    let fbpat = a.read(0x2243c);
    registers.push((0x2243c, fbpat));
    if fbpat < fbps || fbpat > 16 || fbpat % fbps != 0 {
        return Err("invalid FBPA topology");
    }
    let units = fbpat / fbps;
    let mut common = None;
    let mut total = 0;
    for fbp in 0..fbps {
        let mut partition = 0;
        for unit in fbp * units..(fbp + 1) * units {
            if disabled & (1 << unit) != 0 {
                continue;
            }
            let reg = 0x11020c + unit as usize * 4096;
            let mib = a.read(reg);
            registers.push((reg, mib));
            if mib == 0 || mib > 4096 {
                return Err("invalid FBPA capacity");
            }
            partition += u64::from(mib) << 20;
        }
        if partition == 0 {
            continue;
        }
        if common.is_some_and(|c| c != partition) {
            return Err("mixed VRAM outside bootstrap");
        }
        common = Some(partition);
        total += partition;
    }
    if total > 1 << 32
        || total < 32 << 20
        || aperture < 16 << 20
        || aperture >= total
        || !aperture.is_power_of_two()
    {
        return Err("invalid VRAM or BAR aperture");
    }
    let mut references = Vec::new();
    let disp = a.read(0x610010);
    registers.push((0x610010, disp));
    if disp & 15 != 9 {
        return Err("unsupported inherited display instance target");
    }
    reference(&mut references, total, u64::from(disp & !255) << 8, 65536)?;
    for i in 0..9 {
        let reg = 0x610494 + i * 16;
        let push = a.read(reg);
        registers.push((reg, push));
        if push != 0 {
            match push & 3 {
                1 => reference(&mut references, total, u64::from(push & !3) << 8, 4096)?,
                2 | 3 => {}
                _ => return Err("unsupported display push target"),
            }
        }
    }
    for reg in [0x1704, 0x1714] {
        let control = a.read(reg);
        registers.push((reg, control));
        if control & 0x80000000 == 0 {
            continue;
        }
        let inst = u64::from(control & 0x0fffffff) << 12;
        reference(&mut references, total, inst, 4096)?;
        let mut p = Pramin::new(a, total);
        let pd = p.read64(inst + 0x200)?;
        let limit = p.read64(inst + 0x208)?;
        if pd & 4095 != 0 || limit >= 1 << 32 {
            return Err("unsupported inherited BAR page directory");
        }
        let count = (limit >> 27) + 1;
        reference(
            &mut references,
            total,
            pd,
            ((count * 8 + 4095) & !4095).max(4096),
        )?;
        for n in 0..count {
            let entry = p.read64(pd + n * 8)?;
            for (half, page, entries) in [
                (entry as u32, 17u32, 1024u64),
                ((entry >> 32) as u32, 12, 32768),
            ] {
                if half & 3 == 0 {
                    continue;
                }
                if half & 3 != 1 {
                    return Err("non-VRAM BAR page table outside bootstrap");
                }
                let table = u64::from(half & 0xfffffff0) << 8;
                reference(&mut references, total, table, entries * 8)?;
                let used = (((limit - (n << 27)) >> page) + 1).min(entries);
                let mut mapped_floor = total;
                for j in 0..used {
                    let pte = p.read64(table + j * 8)?;
                    if pte & 1 != 0 && (pte >> 33) & 3 == 0 {
                        let address = (pte & 0xffffff00) << 8;
                        if address + (1 << page) > total {
                            return Err("BAR maps beyond discovered VRAM");
                        }
                        if address >= aperture {
                            mapped_floor = mapped_floor.min(address);
                        } else if address + (1 << page) > aperture {
                            return Err("BAR page straddles reservation boundary");
                        }
                    }
                }
                if mapped_floor < total {
                    reference(&mut references, total, mapped_floor, 4096)?;
                }
            }
        }
    }
    // Reserve the firmware-visible low aperture and the whole upper instance
    // envelope, including descendants and firmware's final MiB. No BAR remapping.
    if references
        .iter()
        .any(|r| r.start < aperture && r.end().is_some_and(|e| e > aperture))
    {
        return Err("inherited object straddles reservation boundary");
    }
    let upper = references
        .iter()
        .filter(|r| r.start >= aperture)
        .map(|r| r.start)
        .min()
        .unwrap_or(total)
        .min(total - (1 << 20));
    if upper <= aperture {
        return Err("inherited storage leaves no verified bootstrap interval");
    }
    Ok(Inventory {
        total,
        reserved: vec![
            Range {
                start: 0,
                size: aperture,
            },
            Range {
                start: upper,
                size: total - upper,
            },
        ],
        references,
        registers,
    })
}
pub fn pattern(address: u64, seed: u32, phase: usize) -> u32 {
    ((address / 4) as u32).wrapping_mul(0x9e3779b1) ^ seed ^ if phase == 0 { 0 } else { u32::MAX }
}
pub fn exercise<A: Access>(
    p: &mut Pramin<'_, A>,
    layout: &Layout,
    seed: u32,
    mut observe: impl FnMut(usize, usize, &[u8]),
) -> Result<u64, &'static str> {
    let mut guards = Vec::new();
    for o in &layout.objects {
        for r in [
            Range {
                start: o.range.start - 4096,
                size: 4096,
            },
            Range {
                start: o.range.end().ok_or("object overflow")?,
                size: 4096,
            },
        ] {
            guards.push((r, p.bytes(r)?));
        }
    }
    let mut verified = 0;
    for phase in 0..2 {
        // Fill the entire set before reading it, so one bank/region cannot echo
        // another allocation and pass an immediate write/read loop.
        for o in &layout.objects {
            for address in (o.range.start..o.range.end().ok_or("object overflow")?).step_by(4) {
                p.write(address, pattern(address, seed, phase))?;
            }
        }
        for (index, o) in layout.objects.iter().enumerate().rev() {
            let mut data = Vec::with_capacity(o.range.size as usize);
            for address in (o.range.start..o.range.end().ok_or("object overflow")?).step_by(4) {
                let value = p.read(address)?;
                if value != pattern(address, seed, phase) {
                    return Err("display VRAM full read-back mismatch");
                }
                data.extend_from_slice(&value.to_le_bytes());
            }
            verified += o.range.size;
            observe(phase, index, &data);
        }
        for (r, before) in &guards {
            if p.bytes(*r)? != *before {
                return Err("display VRAM guard changed");
            }
        }
    }
    Ok(verified)
}
