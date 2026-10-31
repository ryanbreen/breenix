use std::{cell::RefCell, collections::BTreeMap};
thread_local! {static DEVICE:RefCell<Device>=RefCell::new(Device::default());}
#[derive(Default)]
struct Device { registers:BTreeMap<usize,u32>, vram:BTreeMap<u64,u32>, ticks:u64,
    release:u64, nonce:u32, epoch:u64, withheld:bool, withheld_flip:bool, userd_alias:Vec<u32> }
mod arch {pub fn counter()->u64 {super::DEVICE.with(|d|{let mut d=d.borrow_mut();d.ticks+=1;d.ticks})}}
struct Registers(usize);
impl Registers {
    fn read(&self,r:usize)->u32 {assert_eq!(self.0,0);DEVICE.with(|d|*d.borrow().registers.get(&r).unwrap_or(&0))}
    fn write(&self,r:usize,v:u32) {assert_eq!(self.0,0);DEVICE.with(|d|{
        let mut d=d.borrow_mut();d.registers.insert(r,if r==0x70000 {0}else{v});
        if r==0x640000 {
            d.registers.insert(0x640004,v);
            if v>2048 && !d.withheld_flip {d.vram.insert(0x4010,1);}
        }
    });}
}
mod memory {
    use super::*;
    pub trait Access {fn read(&mut self,r:usize)->u32;fn write(&mut self,r:usize,v:u32);}
    pub struct Pramin<'a,A:Access>(&'a mut A,u64);
    impl<'a,A:Access> Pramin<'a,A> {
        pub fn new(a:&'a mut A,total:u64)->Self {Self(a,total)}
        pub fn read(&mut self,a:u64)->Result<u32,()> {
            assert!(a<self.1);self.0.read(0x1700);
            Ok(DEVICE.with(|d|*d.borrow().vram.get(&a).unwrap_or(&0)))
        }
        pub fn write(&mut self,a:u64,v:u32)->Result<(),()> {
            assert!(a<self.1);self.0.write(0x1700,0);
            DEVICE.with(|d|{let mut d=d.borrow_mut();d.vram.insert(a,v);
                if a==0x10008c {
                    d.vram.insert(0x100088,v);
                    if !d.withheld {
                        unsafe {
                            (d.release as *mut u32).write_volatile(d.nonce);
                            ((d.release+4) as *mut u32).write_volatile(d.epoch as u32);
                            ((d.release+8) as *mut u32).write_volatile((d.epoch>>32) as u32);
                        }
                    }
                }
            });Ok(())
        }
    }
}
mod gpu_cache {
 pub fn drain_wc() { crate::DEVICE.with(|d| {
  let mut d=d.borrow_mut();let put=d.userd_alias[0x8c/4];
  d.vram.insert(0x10008c,put);d.vram.insert(0x100088,put);
  if !d.withheld {unsafe {
   (d.release as *mut u32).write_volatile(d.nonce);
   ((d.release+4) as *mut u32).write_volatile(d.epoch as u32);
   ((d.release+8) as *mut u32).write_volatile((d.epoch>>32) as u32);
  }}
 }); }
}
// TRANSPORT
fn setup(report:&mut [u32;4])->Queue {
    let mut alias=vec![0u32;128];
    let q=Queue{registers:0,total:1<<24,ring:0x200000,userd:0x100000,pbdma:0,ce_error:0x104908,userd_bar:alias.as_mut_ptr() as usize,
        channel:1,instance:0x400000,report:report.as_mut_ptr() as u64,report_va:0x1000,
        epoch:0x123456789,quarantine:0x800000,parking:vec![]};
    // Snapshot fields used by the preparation-side encoder, excluded above.
    assert_eq!((q.report_va,q.quarantine,q.parking.len()),(0x1000,0x800000,0));
    DEVICE.with(|d|{let mut d=d.borrow_mut();*d=Device::default();d.userd_alias=alias;
        d.registers.insert(0x800008,0x80000400);d.release=q.report;d.nonce=7;d.epoch=q.epoch;});q
}
#[test]
fn complete_and_wrapping_receipts() {
    let mut report=[0;4];let q=setup(&mut report);
    DEVICE.with(|d|{let mut d=d.borrow_mut();d.vram.insert(q.userd+0x88,511);d.vram.insert(q.userd+0x8c,511);d.userd_alias[0x8c/4]=511;});
    assert!(q.drain(100));
    assert!(q.exec(&q.packet(0x300000,25,7).unwrap(),100));
    assert_eq!(report,[7,q.epoch as u32,(q.epoch>>32) as u32,0]);
    DEVICE.with(|d|assert_eq!(d.borrow().vram[&(q.userd+0x8c)],0));
}
#[test]
fn timeout_stale_epoch_and_foreign_channel_refuse() {
    for fault in 0..4 {
        let mut report=[0;4];let q=setup(&mut report);
        DEVICE.with(|d|{let mut d=d.borrow_mut();match fault {
            0=>d.withheld=true,1=>d.epoch-=1,2=>{d.registers.insert(0x800008,0);},
            _=>{d.registers.insert(q.ce_error,1);}}});
        assert!(!q.exec(&q.packet(0x300000,25,7).unwrap(),40));
    }
}
#[test]
fn every_binding_word_and_display_health_is_observed() {
    let mut report=[0;4];let _q=setup(&mut report);
    let d=Display {registers:0,total:1<<24,push:0x3000,notifier:0x4000,head:0,sor:0,
        methods:vec![(0x460,55),(0x468,99),(0x84,0x80000000),(0x80,0)],target:42};
    assert_eq!(d.target,42);
    DEVICE.with(|r|{let mut r=r.borrow_mut();r.registers.extend([(0x640460,55),(0x660460,55),(0x640468,99),(0x660468,99),
        (0x616308,1),(0x61c004,1)]);});
    assert!(d.bound());
    for (register,value) in [(0x640460,54),(0x660460,54),(0x640468,98),(0x660468,98),
        (0x61009c,1),(0x616308,0),(0x61c004,0),(0x61c030,0x10000000)] {
        let old=Registers(0).read(register);
        DEVICE.with(|r|{r.borrow_mut().registers.insert(register,value);});assert!(!d.bound());
        DEVICE.with(|r|{r.borrow_mut().registers.insert(register,old);});
    }
    // An expired terminal deadline cannot publish even a healthy binding.
    assert!(!d.show(0));
}

#[test]
fn flip_uses_preencoded_reserved_half_page_and_waits_for_notifier() {
    for withheld in [false,true] {
        let mut report=[0;4];let _q=setup(&mut report);
        let d=Display {registers:0,total:1<<24,push:0x3000,notifier:0x4010,head:0,sor:0,
            methods:vec![(0x460,55),(0x468,99),(0x84,0x80000010),(0x80,0)],target:42};
        let prepared:Vec<_>=d.methods.iter().flat_map(|&(m,v)|[0x40000|m,v]).collect();
        DEVICE.with(|r|{let mut r=r.borrow_mut();r.withheld_flip=withheld;
            // Ordinary EVO GET is behind PUT; terminal publication must not
            // wait for that queue before appending its prepared flip.
            r.registers.insert(0x640000,24);r.registers.insert(0x640004,0);
            for (i,&v) in prepared.iter().enumerate(){r.vram.insert(d.push+2048+i as u64*4,v);}
            r.registers.extend([(0x640460,55),(0x660460,55),(0x640468,99),(0x660468,99),
                (0x616308,1),(0x61c004,1)]);
        });
        assert_eq!(d.show(200),!withheld);
        DEVICE.with(|r|{let r=r.borrow();
            assert_eq!(r.vram[&(d.push+24)],0x20000800);
            assert_eq!(r.registers[&0x640000],2048+d.methods.len() as u32*8);
            for (i,&v) in prepared.iter().enumerate(){assert_eq!(r.vram[&(d.push+2048+i as u64*4)],v);}
        });
    }
}
