//! Native PRESENT must latch the resource itself before returning its fence.
#![allow(dead_code)] // Hardware modules include APIs beyond this host fixture.
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/memory.rs"] mod memory;
#[path = "../kernel/src/gpu/gk208/vm.rs"] mod vm;
#[path = "../kernel/src/gpu/gk208/resource.rs"] mod resource;
#[path = "../kernel/src/gpu/gk208/copy.rs"] mod copy;
#[path = "../kernel/src/gpu/gk208/present.rs"] mod present;
use std::collections::BTreeMap;
use present::{Surface, Io};

#[derive(Default)]
struct Memory(BTreeMap<u64,u64>);
impl vm::Io for Memory {
    fn read(&mut self,a:u64)->Result<u64,&'static str>{Ok(*self.0.get(&a).unwrap_or(&0))}
    fn write(&mut self,a:u64,v:u64)->Result<(),&'static str>{self.0.insert(a,v);Ok(())}
    fn flush(&mut self)->Result<(),&'static str>{Ok(())}
    fn invalidate(&mut self,_:u64)->Result<u32,&'static str>{Ok(0)}
    fn register(&mut self,_:usize)->u32{0}
}
fn target(format:u32)->Surface {
    Surface {resource:71,address:0x12340000,width:1280,height:1024,
        pitch:5120,block_height:16,format,dma:3}
}
#[test]
fn scanout_allocation_has_display_geometry_and_retains_its_own_vram() {
    let mut a=memory::allocator::Allocator::new(1<<30,&[]).unwrap();
    let baseline=a.stats();let mut io=Memory::default();let mut r=resource::Resources::<()>::new();
    r.create_scanout(&mut a,&mut io,71,0,129,17).unwrap();
    let surface=r.scanout(71).unwrap();let l=r.layout(71).unwrap();
    assert_eq!((l.width,l.height,l.pitch,l.block_height),(129,17,576,4));
    assert_eq!(surface.resource,71);assert_eq!(surface.address%65536,0);
    assert_eq!(surface.dma,3);assert_eq!(surface.dma_words(1<<30).unwrap()[0],0x0fe00049);
    assert_ne!(surface.address,r.address(71).unwrap(),"EVO needs physical VRAM, not the GR VA");
    r.create(&mut a,&mut io,72,1,0,129,17).unwrap();
    assert!(r.scanout(72).is_none(),"unflagged target cannot be bound");
    let hold=r.hold(71).unwrap();r.unref(71).unwrap();r.unref(72).unwrap();
    r.collect(&mut a,&mut io).unwrap();assert!(r.layout(71).is_some());
    drop(hold);r.collect(&mut a,&mut io).unwrap();assert_eq!(a.stats(),baseline);
}
#[test]
fn evo_update_names_the_target_in_both_formats_and_heads() {
    for (format,params) in [(0,0xcf00),(1,0xd500)] {
        for head in [0,1] {
            let methods=target(format).methods(head).unwrap();let h=head*0x300;
            assert_eq!(methods,vec![(0x460+h,0x123400),(0x468+h,0x04000500),
                (0x46c+h,0x5004),(0x470+h,params),(0x474+h,3),
                (0x84,0x80000000),(0x80,0)]);
        }
    }
    assert!(Surface{address:0x12340001,..target(0)}.methods(0).is_err());
    assert!(Surface{format:2,..target(0)}.methods(0).is_err());
    assert!(Surface{pitch:5119,..target(0)}.methods(0).is_err());
}
#[derive(Default)]
struct Display {
    regs:BTreeMap<usize,u32>, methods:Vec<(u32,u32)>, events:Vec<&'static str>,
    notifier:u32,skip:bool,wrong:bool,stale_armed:bool,fail_notifier:bool,
}
impl Io for Display {
    fn clear_notifier(&mut self)->Result<(),&'static str>{self.events.push("clear");self.notifier=0;Ok(())}
    fn submit(&mut self,methods:&[(u32,u32)])->Result<(),&'static str>{
        self.events.push("submit");self.methods=methods.to_vec();
        if !self.skip {
            for &(m,v) in methods {if (0x460..=0x474).contains(&m) {
                self.regs.insert(0x640000+m as usize,v);
                if !self.stale_armed {self.regs.insert(0x660000+m as usize,v);}
            }}
            if self.wrong {self.regs.insert(0x640460,0x4400);}
            self.notifier=1;
        }Ok(())
    }
    fn complete(&mut self)->Result<u32,&'static str>{self.events.push("complete");Ok(if self.fail_notifier{0}else{self.notifier})}
    fn read(&mut self,r:usize)->u32{self.events.push("read");*self.regs.get(&r).unwrap_or(&0)}
}
#[test]
fn flush_requires_a_fresh_notifier_and_both_target_surface_reads() {
    let mut d=Display::default();let s=target(0);
    let receipt=present::flush(&mut d,0,s).unwrap();
    assert_eq!(receipt.notifier,1);assert_eq!(receipt.active,receipt.armed);
    assert_eq!(receipt.active[0],(s.address>>8) as u32);
    assert_eq!(&d.events[..3],&["clear","submit","complete"]);
    for fault in 0..4 {
        let mut d=Display{notifier:1,skip:fault==0,wrong:fault==1,
            stale_armed:fault==2,fail_notifier:fault==3,..Display::default()};
        assert!(present::flush(&mut d,0,s).is_err(),"accepted skipped/wrong/stale flush {fault}");
        assert_eq!(d.events[0],"clear");
    }
    let mut d=Display::default();present::flush(&mut d,0,s).unwrap();
    d.skip=true;assert!(present::flush(&mut d,0,s).is_err(),"old same-surface state cannot replace a flush");
}
