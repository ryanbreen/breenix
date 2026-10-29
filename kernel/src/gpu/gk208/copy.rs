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
// Host method headers: NVIDIA open-gpu-doc classes/host/cla06f.h (MIT).
/*******************************************************************************
    Copyright (c) 2019, NVIDIA CORPORATION. All rights reserved.

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

//! Kernel-only A0B5 pitch and tiled transfers. cla0b5.h and cla06f.h
//! (MIT, pinned in THIRD_PARTY_NOTICES) define the method fields.
use alloc::{vec, vec::Vec};
#[derive(Clone, Copy)]
pub struct Rect {
    pub src: u64,
    pub dst: u64,
    pub width: u32,
    pub height: u32,
    pub src_pitch: u32,
    pub dst_pitch: u32,
}
fn end(a: u64, pitch: u32, width: u32, height: u32) -> Result<u64, &'static str> {
    if width == 0 || height == 0 || width > pitch || height > 65536 {
        return Err("invalid CE rectangle");
    }
    let e = a.checked_add(u64::from(height - 1) * u64::from(pitch))
        .and_then(|v| v.checked_add(u64::from(width))).ok_or("CE extent overflow")?;
    if e > 1 << 40 { return Err("CE extent exceeds GPU VA"); }
    Ok(e)
}
fn method(out: &mut Vec<u32>, address: u32, values: &[u32]) {
    out.push(0x20000000 | ((values.len() as u32) << 16) | (4 << 13) | (address >> 2));
    out.extend_from_slice(values);
}
/// The data launch alone: a flushed, non-pipelined transfer with no release.
pub fn transfer(r: Rect) -> Result<Vec<u32>, &'static str> {
    let s = end(r.src, r.src_pitch, r.width, r.height)?;
    let d = end(r.dst, r.dst_pitch, r.width, r.height)?;
    if r.src < d && r.dst < s { return Err("CE overlap requires staging"); }
    let mut out = vec![];
    method(&mut out, 0, &[0xa0b5]);
    method(&mut out, 0x400, &[(r.src >> 32) as u32, r.src as u32,
        (r.dst >> 32) as u32, r.dst as u32, r.src_pitch, r.dst_pitch, r.width, r.height]);
    method(&mut out, 0x300, &[0x386]);
    Ok(out)
}
/// The three completion words are engine releases, following a non-pipelined
/// flushed transfer; a PFIFO front-end release cannot certify copy completion.
pub fn encode(r: Rect, semaphore: u64, epoch: u64, sequence: u32) -> Result<Vec<u32>, &'static str> {
    if semaphore % 4 != 0 || semaphore > (1 << 40) - 12 || epoch == 0 {
        return Err("invalid CE completion record");
    }
    let mut out = transfer(r)?;
    for (offset, value) in [(4, epoch as u32), (8, (epoch >> 32) as u32), (0, sequence)] {
        release(&mut out, semaphore + offset, value);
    }
    Ok(out)
}

/// A0B5 one-word engine release, ordered after preceding non-pipelined work.
pub fn release(out: &mut Vec<u32>, address: u64, value: u32) {
    method(out, 0x240, &[(address >> 32) as u32, address as u32, value]);
    method(out, 0x300, &[0x0c]);
}

/// Testing controls alter engine releases, with a separate terminal drain word.
#[cfg(any(feature = "testing", feature = "gt710_core_probe"))]
pub fn probe_encode(r: Rect, semaphore: u64, epoch: u64, sequence: u32,
    control: super::core_record::Control,
) -> Result<Vec<u32>, &'static str> {
    use super::core_record::Control;
    if !matches!(control, Control::WithheldRelease | Control::StaleEpoch)
        || semaphore % 16 != 0 || semaphore > (1 << 40) - 16 || epoch == 0 || sequence == 0 {
        return Err("invalid CE control");
    }
    let mut out = transfer(r)?;
    let written_epoch = if control == Control::StaleEpoch { epoch.wrapping_sub(1) } else { epoch };
    release(&mut out, semaphore + 4, written_epoch as u32);
    release(&mut out, semaphore + 8, (written_epoch >> 32) as u32);
    if control != Control::WithheldRelease { release(&mut out, semaphore, sequence); }
    release(&mut out, semaphore + 12, sequence);
    Ok(out)
}

/// Uncompressed Kepler 2D storage, resolved by the resource owner.
#[derive(Clone, Copy)]
pub struct Image {
    pub address: u64,
    pub width: u32,
    pub height: u32,
    pub texel_bytes: u32,
    pub pitch: u32,
    pub block_height: u32,
    pub bytes: u64,
}
#[derive(Clone, Copy)]
pub struct Linear {
    pub address: u64,
    pub bytes: u64,
    pub offset: u64,
    pub stride: u32,
}
#[derive(Clone, Copy)]
enum Surface { Tiled(Image, [u32; 2]), Pitch(Linear) }
#[derive(Clone, Copy)]
struct Blit { src: Surface, dst: Surface, width: u32, height: u32, bpp: u32 }
/// At most two non-pipelined launches. The sole completion follows both.
#[derive(Clone, Copy)]
pub struct Plan { copies: [Option<Blit>; 2], stale_fence: bool }
#[derive(Clone, Copy, Default)]
pub struct Extras { pub skip: bool, pub stale_fence: bool }
impl Plan {
    /// Apply one request's controls to its engine work before publication.
    pub fn with_extras(mut self, x: Extras) -> Self {
        if x.skip { self.copies = [None, None]; }
        self.stale_fence = x.stale_fence;
        self
    }
}
impl Image {
    fn validate(self) -> Result<(), &'static str> {
        if self.width == 0 || self.height == 0 || self.width > 16384 || self.height > 16384
            || !matches!(self.texel_bytes, 1 | 4) || !matches!(self.block_height, 1 | 2 | 4 | 8 | 16) {
            return Err("invalid CE tiled image");
        }
        let rows = self.height.next_multiple_of(8*self.block_height);
        if self.pitch != (self.width * self.texel_bytes).next_multiple_of(64)
            || self.address % 512 != 0 || self.address.checked_add(self.bytes).is_none_or(|e| e > 1 << 40)
            || self.bytes < u64::from(self.pitch)*u64::from(rows) {
            return Err("invalid CE tiled image");
        }
        Ok(())
    }
    fn clip(self, r: [u32; 4]) -> Result<[u32; 4], &'static str> {
        self.validate()?;
        let [x,y,w,h] = r;
        if x >= self.width || y >= self.height || w == 0 || h == 0 { return Err("empty CE image rectangle"); }
        Ok([x,y,w.min(self.width-x),h.min(self.height-y)])
    }
}
impl Linear {
    fn validate(self, width: u32, height: u32, bpp: u32) -> Result<(), &'static str> {
        let row = width.checked_mul(bpp).ok_or("CE row overflow")?;
        let first = self.address.checked_add(self.offset).ok_or("CE backing offset overflow")?;
        let last = end(first, self.stride, row, height)?;
        if self.offset.checked_add(last-first).is_none_or(|e| e > self.bytes) {
            return Err("CE backing too small");
        }
        Ok(())
    }
}
fn host_plan(image: Image, backing: Linear, r: [u32; 4], upload: bool) -> Result<Plan, &'static str> {
    let [x,y,w,h] = image.clip(r)?;
    backing.validate(w,h,image.texel_bytes)?;
    if backing.address < image.address + image.bytes && image.address < backing.address + backing.bytes {
        return Err("CE host/image alias");
    }
    let (tiled,pitch) = (Surface::Tiled(image,[x,y]),Surface::Pitch(backing));
    let (src,dst) = if upload { (pitch,tiled) } else { (tiled,pitch) };
    Ok(Plan { stale_fence: false, copies: [Some(Blit { src,dst,width:w,height:h,bpp:image.texel_bytes }),None] })
}
pub fn upload(image: Image, backing: Linear, r: [u32; 4]) -> Result<Plan, &'static str> {
    host_plan(image,backing,r,true)
}
pub fn readback(image: Image, backing: Linear, r: [u32; 4]) -> Result<Plan, &'static str> {
    host_plan(image,backing,r,false)
}
/// Clip both ends together and route aliased rectangles through staging.
pub fn region(src: Image, dst: Image, r: [u32; 6], staging: Option<Image>) -> Result<Plan, &'static str> {
    let [sx,sy,w,h,dx,dy] = r;
    let [_,_,sw,sh] = src.clip([sx,sy,w,h])?;
    let [_,_,dw,dh] = dst.clip([dx,dy,w,h])?;
    if src.texel_bytes != dst.texel_bytes { return Err("CE copy format size mismatch"); }
    let (w,h) = (sw.min(dw),sh.min(dh));
    let blit = |s,d| Blit { src:s,dst:d,width:w,height:h,bpp:src.texel_bytes };
    let (s,d) = (Surface::Tiled(src,[sx,sy]),Surface::Tiled(dst,[dx,dy]));
    let alias = src.address < dst.address + dst.bytes && dst.address < src.address + src.bytes;
    // Conservatively stage copies within one allocation, including disjoint
    // rectangles, so this path needs no assumptions about tiled fetch granularity.
    if alias {
        let t = staging.ok_or("CE overlap requires VRAM staging")?;
        t.validate()?;
        if t.texel_bytes != src.texel_bytes || t.width < w || t.height < h
            || (t.address < src.address+src.bytes && src.address < t.address+t.bytes)
            || (t.address < dst.address+dst.bytes && dst.address < t.address+t.bytes) {
            return Err("invalid CE overlap staging");
        }
        let t = Surface::Tiled(t,[0,0]);
        Ok(Plan { stale_fence: false, copies:[Some(blit(s,t)),Some(blit(t,d))] })
    } else { Ok(Plan { stale_fence: false, copies:[Some(blit(s,d)),None] }) }
}
fn surface(out: &mut Vec<u32>, s: Surface, source: bool) -> (u64,u32,u32) {
    match s {
        Surface::Pitch(l) => (l.address+l.offset,l.stride,if source {1<<7} else {1<<8}),
        Surface::Tiled(i,[x,y]) => {
            // cla0b5.h: FERMI_8 GOB height, one-GOB width/depth, pixel
            // dimensions/origins when REMAP_ENABLE supplies component size.
            method(out,if source {0x728} else {0x70c},
                &[0x1000 | (i.block_height.trailing_zeros()<<4),i.width,i.height,1,0,x | (y<<16)]);
            (i.address,0,0)
        }
    }
}
pub fn encode_plan(plan: Plan, semaphore: u64, epoch: u64, sequence: u32) -> Result<Vec<u32>, &'static str> {
    let record_bytes = if plan.stale_fence { 16 } else { 12 };
    if semaphore % 4 != 0 || semaphore > (1<<40)-record_bytes || epoch == 0 || sequence == 0 {
        return Err("invalid CE completion record");
    }
    let mut out = vec![];
    method(&mut out,0,&[0xa0b5]);
    for b in plan.copies.into_iter().flatten() {
        let (s,sp,sl) = surface(&mut out,b.src,true);
        let (d,dp,dl) = surface(&mut out,b.dst,false);
        // One component per pixel, identity SRC_X -> DST_X. Component size
        // is bytes minus one; line length/origins are therefore in pixels.
        method(&mut out,0x708,&[(b.bpp-1)<<16]);
        method(&mut out,0x400,&[(s>>32) as u32,s as u32,(d>>32) as u32,d as u32,sp,dp,b.width,b.height]);
        method(&mut out,0x300,&[0x606 | sl | dl]);
    }
    let echo = if plan.stale_fence { sequence.wrapping_add(1) } else { sequence };
    for (off,value) in [(4,epoch as u32),(8,(epoch>>32) as u32),(0,echo)] {
        release(&mut out,semaphore+off,value);
    }
    if plan.stale_fence {
        // This distinct engine release certifies drain, not the bad fence echo.
        release(&mut out,semaphore+12,sequence);
    }
    Ok(out)
}
