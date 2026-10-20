//! GPU contract fixtures, independent of the rendering implementation and its expected pixels.
use crate::*;
use libbreenix::gpu::{self, abi as ga};
use ga::Plain;

pub(crate) fn counters() -> Result<ga::Counters, String> {
    let (r,c) = gpu::counters(0);
    if r == 0 { Ok(c) } else { Err(format!("gpu COUNTERS {}", outcome(r))) }
}

/// Case-owned device resources; all handles are released even when an assertion fails.
struct Fixture { context: bgl::Context, target: u64, texture: u64 }
impl Fixture {
    fn new(w:u32,h:u32) -> Result<Self,String> {
        let context=bgl::Context::new().map_err(|e|format!("CONTEXT_CREATE {}",outcome(e)))?;
        let target=gpu::target_create(ga::format::B8G8R8A8,w,h).map_err(|e|format!("TARGET_CREATE {}",outcome(e)))?;
        let texture=match gpu::texture_create(ga::format::B8G8R8A8,w,h) {
            Ok(t)=>t,Err(e)=>{gpu::resource_destroy(target);return Err(format!("TEXTURE_CREATE {}",outcome(e)));}
        };
        Ok(Self{context,target,texture})
    }
    fn clear(&mut self,w:u32,h:u32,colour:u32)->Result<(),String>{
        self.context.begin(self.target,w,h);self.context.clear(colour);
        self.context.submit().and_then(|f|f.wait(2_000_000_000)).map_err(|e|format!("draw fence {}",outcome(e)))
    }
    fn pixels(&self,w:u32,h:u32,stride:u32)->Result<Vec<u8>,String>{
        let mut bytes=vec![0xA5;stride as usize*h as usize];let mut record=[0;ga::record::SIZE];
        let r=gpu::readback(self.target,[0,0,w,h],&mut bytes,stride,&mut record);
        if r==0 {Ok(bytes)}else{Err(format!("READBACK {}",outcome(r)))}
    }
}
impl Drop for Fixture {
    fn drop(&mut self){gpu::resource_destroy(self.texture);gpu::resource_destroy(self.target);}
}

// gpu-raster: §3 replaces CPU surface guards with user-copy guards and a canary device target.
pub(crate) fn guard_pages()->CaseResult {
    let mut f=Fixture::new(8,4).cite("3")?;
    let mut canary=Fixture::new(8,4).cite("3")?;
    canary.clear(8,4,0xff634221).cite("3")?;
    let before=canary.pixels(8,4,32).cite("3")?;
    let page=unsafe{memory::mmap(core::ptr::null_mut(),3*4096,memory::PROT_READ|memory::PROT_WRITE,
        memory::MAP_PRIVATE|memory::MAP_ANONYMOUS,-1,0)}.cite("3")?;
    let result=(||->CaseResult{
        // SAFETY: the mapping has three pages; only the middle page is a user buffer.
        unsafe {memory::mprotect(page,4096,memory::PROT_NONE)}.cite("3")?;
        unsafe {memory::mprotect(page.add(8192),4096,memory::PROT_NONE)}.cite("3")?;
        let end=unsafe{page.add(8192)} as u64;
        for rect in [[0,0,8,4],[u32::MAX,0,8,4],[0,u32::MAX,8,4],[0,0,u32::MAX,u32::MAX]] {
            let (r,_)=gpu::upload_raw(f.context.handle(),f.texture,rect,end-127,128,32,
                ga::upload_source::B8G8R8A8,ga::purpose::FIXTURE);
            ensure(r== -14 || r== -22,"3, 4","gpu-raster: guarded/extreme UPLOAD was not refused with EFAULT/EINVAL")?;
            let mut rb=ga::Readback{target:f.target,x:rect[0],y:rect[1],width:rect[2],height:rect[3],
                pixels:end-127,pixels_len:128,stride:32,..ga::Readback::ZERO};
            let mut record=[0u8;ga::record::SIZE];rb.record=record.as_mut_ptr() as u64;
            let r=gpu::call_with(ga::op::READBACK,&mut rb);
            ensure(r== -14 || r== -22,"3, 4","gpu-raster: guarded/extreme READBACK was not refused with EFAULT/EINVAL")?;
        }
        f.clear(8,4,0xff112233).cite("3")?;
        ensure(canary.pixels(8,4,32).cite("3")?==before,"3","gpu-raster: draw changed the adjacent canary target")
    })();
    let _=memory::munmap(page,3*4096);result
}

// gpu-raster: padding belongs to the caller on both sides of a GPU transfer.
pub(crate) fn stride_padding()->CaseResult {
    let mut f=Fixture::new(14,18).cite("3")?;
    for (source,bpp) in [(ga::upload_source::B8G8R8A8,4usize),(ga::upload_source::R8G8B8,3)] {
        let stride=14*bpp+20;let mut bytes=vec![0xA5;stride*18];
        for y in 0..18 {for x in 0..14 {let p=y*stride+x*bpp;
            bytes[p..p+bpp].copy_from_slice(if bpp==4 {&[33,66,99,255]}else{&[99,66,33]});
        }}
        let unchanged=bytes.clone();
        let (r,_)=gpu::upload(f.context.handle(),f.texture,[0,0,14,18],&bytes,stride as u32,source,ga::purpose::FIXTURE);
        ensure(r==0,"3","gpu-raster: padded UPLOAD failed")?;
        ensure(bytes==unchanged,"3","gpu-raster: UPLOAD changed its source or padding")?;
        f.context.begin(f.target,14,18);f.context.copy(f.texture,&[[0,0,14,18,0,0]]);
        f.context.submit().and_then(|s|s.wait(2_000_000_000)).map_err(|r|format!("gpu-raster: copy {}",outcome(r))).cite("3")?;
        let pixels=f.pixels(14,18,76).cite("3")?;
        for row in pixels.chunks_exact(76) {
            ensure(row[..56].chunks_exact(4).all(|p|p==[33,66,99,255]),"3","gpu-raster: padded transfer changed a pixel")?;
            ensure(row[56..].iter().all(|&p|p==0xA5),"3","gpu-raster: READBACK overwrote row padding")?;
        }
    }Ok(())
}

// gpu-raster: three-byte pixels are transfer formats, never render targets (§2).
pub(crate) fn bpp3_bpp4()->CaseResult {
    let mut f=Fixture::new(14,18).cite("2")?;
    let rgb:Vec<u8>=(0..14*18).flat_map(|n|[(n%251) as u8,73,29]).collect();
    let (r,_)=gpu::upload(f.context.handle(),f.texture,[0,0,14,18],&rgb,42,ga::upload_source::R8G8B8,ga::purpose::FIXTURE);
    ensure(r==0,"2","gpu-raster: three-byte RGB upload failed")?;
    f.context.begin(f.target,14,18);f.context.copy(f.texture,&[[0,0,14,18,0,0]]);
    f.context.submit().and_then(|s|s.wait(2_000_000_000)).map_err(|e|format!("gpu-raster: conversion fence {}",outcome(e))).cite("2")?;
    let mut bgra=f.pixels(14,18,56).cite("2")?;
    ensure(bgra.chunks_exact(4).zip(rgb.chunks_exact(3)).all(|(b,r)|b==[r[2],r[1],r[0],255]),"2","gpu-raster: three-byte upload changed channel order or alpha")?;
    // Exercise the public pixel-format conversion on the device's actual readback, with
    // a three-byte destination and row sentinels, rather than drawing a three-byte target.
    let source=unsafe{FrameBuf::from_raw(bgra.as_mut_ptr(),14,18,56,4,true)}.cite("2")?;
    let mut rgb_out=vec![0xA5;48*18];
    let mut dest=unsafe{FrameBuf::from_raw(rgb_out.as_mut_ptr(),14,18,48,3,false)}.cite("2")?;
    libgfx::blit::blit(&mut dest,&source,0,0);
    for (n,row) in rgb_out.chunks_exact(48).enumerate(){
        ensure(row[..42]==rgb[n*42..(n+1)*42] && row[42..].iter().all(|&b|b==0xA5),"2","gpu-raster: three-byte readback conversion changed a pixel or padding")?;
    }Ok(())
}
