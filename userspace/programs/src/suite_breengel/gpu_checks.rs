//! GPU contract fixtures, independent of the rendering implementation and its expected pixels.
use crate::*;
use libbreenix::gpu::{self, abi as ga};
use ga::Plain;
use gpu::upload_audit as uploads;

pub(crate) fn counters() -> Result<ga::Counters, String> {
    let (r,c) = gpu::counters(0);
    if r == 0 { Ok(c) } else { Err(format!("gpu COUNTERS {}", outcome(r))) }
}

/// Case-owned device resources with handle cleanup in Drop.
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
    let page=memory::mmap(core::ptr::null_mut(),3*4096,memory::PROT_READ|memory::PROT_WRITE,
        memory::MAP_PRIVATE|memory::MAP_ANONYMOUS,-1,0).cite("3")?;
    let result=(||->CaseResult{
        // SAFETY: the mapping has three pages; only the middle page is a user buffer.
        memory::mprotect(page,4096,memory::PROT_NONE).cite("3")?;
        memory::mprotect(unsafe{page.add(8192)},4096,memory::PROT_NONE).cite("3")?;
        let end=unsafe{page.add(8192)} as u64;
        for pointer in [unsafe{page.add(4095)} as u64,end-127] {
        for rect in [[0,0,8,4],[u32::MAX,0,8,4],[0,u32::MAX,8,4],[0,0,u32::MAX,u32::MAX]] {
            let expected=if rect[0]==0 && rect[1]==0 {-14}else{-22};
            let (r,_)=gpu::upload_raw(f.context.handle(),f.texture,rect,pointer,128,32,
                ga::upload_source::B8G8R8A8,ga::purpose::FIXTURE);
            ensure(r==expected,"3, 4","gpu-raster: guarded/extreme UPLOAD was not refused with EFAULT/EINVAL")?;
            ensure(canary.pixels(8,4,32).cite("3")?==before,"3","gpu-raster: rejected UPLOAD changed the canary")?;
            let mut rb=ga::Readback{target:f.target,x:rect[0],y:rect[1],width:rect[2],height:rect[3],
                pixels:pointer,pixels_len:128,stride:32,..ga::Readback::ZERO};
            let mut record=[0u8;ga::record::SIZE];rb.record=record.as_mut_ptr() as u64;
            let r=gpu::call_with(ga::op::READBACK,&mut rb);
            let expected=if rect==[0,0,8,4]{-14}else{-22};
            ensure(r==expected,"3, 4","gpu-raster: guarded/extreme READBACK was not refused with EFAULT/EINVAL")?;
        }
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
        declare_fixture(if bpp==4 { &PAD_BGRA } else { &PAD_RGB });
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

// gpu-raster: three-byte upload and readback conversion (§2).
pub(crate) fn bpp3_bpp4()->CaseResult {
    let mut f=Fixture::new(14,18).cite("2")?;
    declare_fixture(&RGB_TABLE);
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
    libgfx_ref::blit::blit(&mut dest,&source,0,0);
    for (n,row) in rgb_out.chunks_exact(48).enumerate(){
        ensure(row[..42]==rgb[n*42..(n+1)*42] && row[42..].iter().all(|&b|b==0xA5),"2","gpu-raster: three-byte readback conversion changed a pixel or padding")?;
    }Ok(())
}

// gpu-raster: §5.9 uploads whole pixels and clips the GPU copy at the destination row.
pub(crate) fn copy_row_clip()->CaseResult {
    let mut f=Fixture::new(8,4).cite("5.9")?;
    declare_fixture(&COPY_TABLE);
    let pixels=[17,34,51,255].repeat(8*4);
    let (r,_)=gpu::upload(f.context.handle(),f.texture,[0,0,8,4],&pixels,32,ga::upload_source::B8G8R8A8,ga::purpose::FIXTURE);
    ensure(r==0,"5.9","gpu-raster: upload fixture failed")?;
    f.clear(8,4,0xff000000).cite("5.9")?;
    f.context.begin(f.target,8,4);f.context.copy(f.texture,&[[5,1,13,2,0,0],[8,2,16,3,0,0]]);
    f.context.submit().and_then(|s|s.wait(2_000_000_000)).map_err(|e|format!("gpu-raster: row copy {}",outcome(e))).cite("5.9")?;
    let actual=f.pixels(8,4,32).cite("5.9")?;
    for y in 0..4 {for x in 0..8 {
        let want=if y==1 && x>=5 {[17,34,51,255]}else{[0,0,0,255]};
        ensure(actual[(y*8+x)*4..(y*8+x+1)*4]==want,"5.9",format!("gpu-raster: clipped uploaded row wrapped or missed pixel {x},{y}"))?;
    }}Ok(())
}

/// Kernel counters bound instances and reconcile successful observed uploads.
fn accounting_refusal(before:&ga::Counters,after:&ga::Counters,mark:uploads::Mark,instances:u64)->Option<&'static str>{
    uploads::refusal(mark,after.uploads_image-before.uploads_image,after.uploads_fixture-before.uploads_fixture)
        .or_else(||(after.instances-before.instances>instances).then_some("cpu-instances"))
}

pub(crate) fn enforce_accounting(before:&ga::Counters,after:&ga::Counters,mark:uploads::Mark,instances:u64)->CaseResult{
    ensure(accounting_refusal(before,after,mark,instances).is_none(),"7",
        format!("gpu-raster: runtime accounting refused {:?}; {}",accounting_refusal(before,after,mark,instances),uploads::hashes()))
}

// gpu-raster: §2.4 each runtime planting must be refused by the same accounting guard.
pub(crate) fn cpu_render_planted()->CaseResult {
    let mut f=Fixture::new(8,4).cite("7")?;
    let mut rendered=vec![0;128];for pixel in rendered.chunks_exact_mut(4){pixel.copy_from_slice(&[17,34,51,255]);}
    for (purpose,reason) in [(ga::purpose::IMAGE,"cpu-image"),(ga::purpose::FIXTURE,"passing-oracle")] {
        let before=counters().cite("7")?; let mark=uploads::mark();
        let (ret,_)=gpu::upload(f.context.handle(),f.texture,[0,0,8,4],&rendered,32,ga::upload_source::B8G8R8A8,purpose);
        ensure(ret==0,"7","gpu-raster: planted upload did not execute; no negative control was exercised")?;
        let after=counters().cite("7")?;
        ensure(accounting_refusal(&before,&after,mark,1)==Some(reason),"7",format!("gpu-raster: CPU upload was not refused as {reason}"))?;
    }
    let before=counters().cite("7")?; let mark=uploads::mark();
    f.context.begin(f.target,8,4);
    let per_pixel:Vec<_>=(0..4).flat_map(|y|(0..8).map(move|x|[x,y,x+1,y+1])).collect();
    f.context.solid(0xff112233,&per_pixel);
    f.context.submit().and_then(|f|f.wait(2_000_000_000)).map_err(|e|format!("planted draw {}",outcome(e))).cite("7")?;
    let after=counters().cite("7")?;
    ensure(accounting_refusal(&before,&after,mark,1)==Some("cpu-instances"),"7","gpu-raster: per-pixel instances escaped the runtime guard")
}

// Static input tables for transfer tests, independent of the destination image.
const fn repeat_pixel<const N:usize>(pixel:[u8;4],bpp:usize)->[u8;N]{
    let mut out=[0;N];let mut i=0;while i<N{out[i]=pixel[i%bpp];i+=1;}out
}
static PAD_BGRA:[u8;14*18*4]=repeat_pixel([33,66,99,255],4);
static PAD_RGB:[u8;14*18*3]=repeat_pixel([99,66,33,0],3);
static COPY_TABLE:[u8;8*4*4]=repeat_pixel([17,34,51,255],4);
static RGB_TABLE:[u8;14*18*3]={
    let mut out=[0;14*18*3];let mut i=0;while i<14*18{out[i*3]=(i%251) as u8;out[i*3+1]=73;out[i*3+2]=29;i+=1;}out
};
pub(crate) fn declare_fixture(table:&'static [u8]){uploads::declare(table);}
pub(crate) fn audit_begin()->Option<(ga::Counters,uploads::Mark)>{
    let name=libbreenix::suite::running_case()?;
    if !name.starts_with("raster/"){return None;}
    uploads::start();
    declare_fixture(&font5x7::FONT_TABLE);
    // The dependency stores coverage cells as static rows. Declare each complete size
    // table, including replacement, before rendering.
    use noto_sans_mono_bitmap::{get_raster,FontWeight,RasterHeight};
    for height in [RasterHeight::Size16,RasterHeight::Size32] {
        let mut rows=Vec::new();
        for ch in (32u8..=126).map(char::from).chain(core::iter::once('\u{fffd}')) {
            if let Some(r)=get_raster(ch,FontWeight::Regular,height){rows.extend_from_slice(r.raster());}
        }
        uploads::declare_rows(rows);
    }
    counters().ok().map(|c|(c,uploads::mark()))
}
/// Record each case/isolated child before stage transport and failure artifacts.
pub(crate) fn audit_finish(before:Option<(ga::Counters,uploads::Mark)>){
    let Some(name)=libbreenix::suite::running_case() else{return;};
    if !name.starts_with("raster/"){return;}
    uploads::stop();
    let Ok(directory)=std::env::var("BREENIX_RASTER_AUDIT")else{return;};
    static NEXT:AtomicU64=AtomicU64::new(0);
    let Ok(pid)=process::getpid()else{return;};
    let file=format!("{directory}/{}-{}",pid.raw(),NEXT.fetch_add(1,Relaxed));
    let record=match (before,counters()) {
        (Some((b,mark)),Ok(a))=>format!("{name}\t{}\t{}\t{}\t{}\t{}\n",
            uploads::refusal(mark,a.uploads_image-b.uploads_image,a.uploads_fixture-b.uploads_fixture).unwrap_or("ok"),
            a.instances-b.instances,a.draws-b.draws,
            u8::from(LAST_JUDGED.lock().unwrap_or_else(|e|e.into_inner()).is_some()),uploads::hashes()),
        _=>format!("{name}\tERROR gpu-raster: COUNTERS unavailable\n"),
    };
    let _=std::fs::write(file,record);
}
// Aggregate actual execution, including children, without repeating cases.
pub(crate) fn audit_category(expected:&[Case])->CaseResult{
    if let Ok(error)=std::env::var("BREENIX_RASTER_AUDIT_ERROR"){return at("7",error);}
    let directory=std::env::var("BREENIX_RASTER_AUDIT").map_err(|e|format!("gpu-raster: accounting directory: {e}"))?;
    struct Cleanup(String);
    impl Drop for Cleanup {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);}}
    let _cleanup=Cleanup(directory.clone());
    let mut results:std::collections::BTreeMap<String,(String,u64,bool)>=std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(directory).map_err(|e|e.to_string())? {
        let line=std::fs::read_to_string(entry.map_err(|e|e.to_string())?.path()).map_err(|e|e.to_string())?;
        let fields:Vec<_>=line.trim().split('\t').collect();
        ensure(fields.len()==6,"7",format!("gpu-raster: malformed accounting record {line}"))?;
        let r=results.entry(fields[0].to_string()).or_default();
        if fields[1]!="ok" {r.0=format!("{}; {}",fields[1],fields[5]);}
        r.1+=fields[3].parse::<u64>().map_err(|e|e.to_string())?;r.2|=fields[4]=="1";
    }
    for case in expected.iter().filter(|c|!matches!(c.id,"cpu-pixels-zero"|"cpu-render-planted")) {
        let key=format!("raster/{}",case.id);let r=results.get(&key).ok_or_else(||format!("gpu-raster: {key} produced no accounting record"))?;
        ensure(r.0.is_empty(),"7",format!("gpu-raster: {key} refused {}",r.0))?;
        ensure(!r.2 || r.1>0,"7",format!("gpu-raster: {key} judged drawing without any GPU draw"))?;
    }Ok(())
}
