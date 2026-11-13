//! Monitor modes use unity viewports inside the retained EDID raster.
#![allow(dead_code)] // Host includes the production hardware module's other APIs.
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/edid.rs"] mod edid;
#[path = "../kernel/src/gpu/gk208/modeset.rs"] mod modeset;
use std::collections::BTreeMap;
fn raster() -> edid::Timing {
    edid::Timing {width:1280,height:1024,clock_khz:108000,hsync_start:1328,
        hsync_end:1440,htotal:1688,vsync_start:1025,vsync_end:1028,vtotal:1066,flags:30}
}
#[derive(Default)]
struct Registers {words:BTreeMap<usize,u32>,reads:Vec<usize>}
impl modeset::Io for Registers {
    fn read(&mut self,r:usize)->u32 {self.reads.push(r);*self.words.get(&r).unwrap_or(&0)}
    fn write(&mut self,r:usize,v:u32){self.words.insert(r,v);}
    fn delay_us(&mut self,_:u32){}
}
#[test]
fn both_modes_encode_surface_viewports_and_fixed_edid_timing() {
    for head in [0,1,3] {for (width,height) in [(1024,768),(1280,1024),(2560,2048)] {
        let h=head*0x300;
        let mut timing=raster();
        if width==1024 {timing=edid::Timing{width:1024,height:768,clock_khz:65000,
            hsync_start:1048,hsync_end:1184,htotal:1344,vsync_start:771,vsync_end:777,vtotal:806,flags:24};}
        let (rw,rh)=(timing.width,timing.height);
        let pitch=width*4;
        let view=modeset::Viewport::new(&timing,width,height).unwrap();
        let methods=view.methods(head).unwrap();
        let m:BTreeMap<_,_>=methods.iter().copied().collect();
        assert_eq!(m[&(0x494+h)],0); // nouveau head907d_view: one tap, no bias
        assert_eq!(m[&(0x4b0+h)],0);
        assert_eq!(m[&(0x4b8+h)],width|height<<16);
        assert_eq!(m[&(0x4bc+h)],0); // hardware centres the viewport; no adjustment
        for r in [0x4c0,0x4c4,0x4c8] {assert_eq!(m[&(r+h)],rw|rh<<16);}
        assert_eq!(m[&(0x42c+h)],0); // black default base/borders
        let encoded=modeset::logical_head_methods(&timing,head,0,1,0x10000000,
            pitch.into(),width,height).unwrap();
        let full:BTreeMap<_,_>=encoded.into_iter().collect();
        assert_eq!(full[&(0x468+h)],width|height<<16);
        assert_eq!(full[&(0x46c+h)],0x01000000|pitch);
        assert_eq!(full[&(0x414+h)],timing.htotal|timing.vtotal<<16);
        assert_eq!(full[&(0x450+h)],timing.clock_khz*1000);
        assert_eq!(full[&(0x420+h)],(timing.htotal-timing.hsync_start+timing.width-1)|(timing.vtotal-timing.vsync_start+timing.height-1)<<16);
        for (r,v) in methods {assert_eq!(full[&r],v);}
        assert_eq!((raster().height-1024)/2,0);
        let mut io=Registers::default();
        for (&method,&value) in &m {for bank in [0x640000,0x660000] {
            io.words.insert(bank+method as usize,value);
        }}
        view.verify(&mut io,head).unwrap();
        for method in [0x494,0x4b8,0x4bc,0x4c0,0x4c4,0x4c8] {
            for bank in [0x640000,0x660000] {
                let r=bank+(method+h) as usize;let old=io.words[&r];
                io.words.insert(r,old^1);
                assert!(view.verify(&mut io,head).is_err(),"stale active/armed viewport accepted");
                io.words.insert(r,old);
            }
        }
        assert!(io.reads.contains(&(0x6604b8+h as usize)));
    }}
}
#[test]
fn refuses_unadmitted_ratios_and_out_of_range_method_fields() {
    // No extrapolation from the two G20 ratios to an unproven scaler range.
    for (w,h) in [(0,800),(1280,0),(3840,2400),(2560,800),(1280,1600),
        (65536,800),(1280,32768)] {
        assert!(modeset::Viewport::new(&raster(),w,h).is_err(),"accepted {w}x{h}");
    }
    let mut small=raster();small.width=1024;small.height=768;
    assert!(modeset::Viewport::new(&small,2560,1600).is_err());
    assert!(modeset::Viewport::new(&raster(),1280,1024).unwrap().methods(4).is_err());
    assert!(modeset::logical_head_methods(&raster(),0,0,1,0x10000000,5120,2560,1600).is_err());
}
