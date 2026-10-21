//! Encoder checks run in a process without a graphics or upload dependency.
#[path="../../../libs/libimg/tests/checks.rs"]
mod checks;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
struct Counting;
static CALLS:AtomicU64=AtomicU64::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self,l:Layout)->*mut u8 {CALLS.fetch_add(1,Relaxed);System.alloc(l)}
    unsafe fn alloc_zeroed(&self,l:Layout)->*mut u8 {CALLS.fetch_add(1,Relaxed);System.alloc_zeroed(l)}
    unsafe fn realloc(&self,p:*mut u8,l:Layout,n:usize)->*mut u8 {CALLS.fetch_add(1,Relaxed);System.realloc(p,l,n)}
    unsafe fn dealloc(&self,p:*mut u8,l:Layout) {System.dealloc(p,l)}
}
#[global_allocator]
static ALLOCATOR:Counting=Counting;
fn limits()->Result<(),String> {
    for (w,h) in [(16385,1),(1,16385),(4097,4097)] {
        let image=libimg::png::Image {width:w,height:h,pixels:Vec::new()};
        let before=CALLS.load(Relaxed);
        let result=libimg_encode::encode(&image);
        let allocations=CALLS.load(Relaxed)-before;
        if result!=Err(libimg::png::PngError::TooLarge) || allocations!=0 {
            return Err(format!("oversized encode {w}x{h}: result {:?}, {allocations} allocations before refusal",result.map(|b|b.len())));
        }
    }
    Ok(())
}
fn number(out:&mut Vec<u8>,n:usize) {out.extend_from_slice(&(n as u64).to_le_bytes());}
fn bytes(out:&mut Vec<u8>,b:&[u8]) {number(out,b.len());out.extend_from_slice(b);}
fn main() {
    let args:Vec<_>=std::env::args().collect();
    let name=args.get(1).expect("check name");let path=args.get(2).expect("result path");
    let result=if !name.starts_with("png_encode_") {Err("encoder check required".into())} else {
        checks::CHECKS.iter().find(|(id,_)|id==name).ok_or("unknown encoder check".to_string())
            .and_then(|(_,run)|run()).and_then(|()|if name=="png_encode_limits" {limits()}else{Ok(())})
    };
    let mut out=Vec::new();bytes(&mut out,result.as_ref().err().map_or(&[][..],|s|s.as_bytes()));
    let images=checks::take_judged_images();number(&mut out,images.len());
    for frame in images {
        bytes(&mut out,frame.label.as_bytes());
        for image in [frame.actual,frame.expected] {
            number(&mut out,image.width as usize);number(&mut out,image.height as usize);bytes(&mut out,&image.pixels);
        }
    }
    std::fs::write(path,out).expect("publish encoder check result");
    std::process::exit(if result.is_ok() {0}else{1});
}
