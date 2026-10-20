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
