extern crate alloc;
#[path="../kernel/src/gpu/gk208/thermal.rs"] mod thermal;
use std::collections::BTreeMap;
use thermal::{Access,Write};
struct Io {r:BTreeMap<u32,u32>, writes:Vec<Write>, sleeps:Vec<u32>, fail_restore:bool, no_echo:bool, notes:Vec<Result<u32,&'static str>>}
impl Io {
    fn new()->Self {Self {r:BTreeMap::from([(0x22400,0),(0x21000,0xc0040000),(0x212a8,1),
        (0x20008,0x12348000),(0x2000c,0x92340003),(0x20400,47)]),writes:vec![],sleeps:vec![],fail_restore:false,no_echo:false,notes:vec![]}}
}
impl Access for Io {
    fn read(&mut self,a:u32)->u32 {*self.r.get(&a).expect("unexpected register")}
    fn write(&mut self,a:u32,v:u32) {if !(a==0x21000 && ((self.fail_restore && v==0xc0040000) || (self.no_echo && v==0xc0040001))) {self.r.insert(a,v);}}
    fn record(&mut self,w:Write) {self.writes.push(w)}
    fn fuse_note(&mut self,result:Result<u32,&'static str>) {self.notes.push(result)}
    fn sleep_ms(&mut self,n:u32) {self.sleeps.push(n)}
}
#[test] fn setup_retains_sensor_controls_but_restores_fuse_access_before_them() {
    let mut io=Io::new();thermal::setup(&mut io).unwrap();
    let expected=[(0x22400,0,0x800,false),(0x21000,0xc0040000,0xc0040001,false),
        (0x21000,0xc0040001,0xc0040000,false),(0x22400,0x800,0,false),
        (0x20008,0x12348000,0x92340000,true),(0x2000c,0x92340003,0x12340000,true)];
    assert_eq!(io.writes.len(),expected.len());
    for (w,(a,b,v,retained)) in io.writes.iter().zip(expected) {
        assert_eq!((w.address,w.before,w.written,w.readback,w.retained),(a,b,v,v,retained));
    }
    assert_eq!(io.sleeps,[20]);
    let sample=thermal::temperature(&mut io);assert_eq!(sample,Ok(47));
    assert_eq!(io.writes.len(),6); // hardware calibration does not need another fuse transaction
    assert_eq!(io.r[&0x22400],0);assert_eq!(io.r[&0x21000],0xc0040000);
    assert_eq!(io.r[&0x20008],0x92340000);assert_eq!(io.r[&0x2000c],0x12340000);
}
#[test] fn unavailable_sensor_and_failed_restore_are_reported_with_cleanup() {
    let mut io=Io::new();io.r.insert(0x212a8,0);
    assert_eq!(thermal::setup(&mut io),Ok(()));
    assert_eq!(io.writes.len(),6);assert_eq!(io.sleeps,[20]);
    assert_eq!(thermal::temperature(&mut io),Ok(47));
    assert_eq!(io.notes,[Ok(0)]);
    assert_eq!(io.r[&0x21000],0xc0040000);assert_eq!(io.r[&0x22400],0);
    let mut io=Io::new();io.fail_restore=true;
    assert_eq!(thermal::setup(&mut io),Ok(()));
    assert_eq!(io.notes,[Err("fuse-restore-failed")]);
    assert_eq!(io.writes.len(),6);assert_eq!(io.writes[2].readback,0xc0040001);
    assert_eq!(io.r[&0x22400],0);assert_eq!(io.sleeps,[20]);
    assert_eq!(thermal::temperature(&mut io),Ok(47));
    let mut io=Io::new();io.r.insert(0x20400,0xffffffff);
    assert_eq!(thermal::temperature(&mut io),Err("sensor-read-error"));
}
#[test] fn v24_downclock_table_and_invalid_bounds() {
    let mut rom=vec![0;1024];rom[0x40..0x4c].copy_from_slice(&[255,184,66,73,84,0,0,1,12,6,1,0]);
    rom[0x4b]=0u8.wrapping_sub(rom[0x40..0x4c].iter().fold(0u8,|s,b|s.wrapping_add(*b)));
    rom[0x4c..0x52].copy_from_slice(&[b'P',2,20,0,0x60,0]);
    rom[0x70..0x74].copy_from_slice(&0x100u32.to_le_bytes());
    rom[0x100..0x104].copy_from_slice(&[0x24,4,3,4]);
    rom[0x104..0x110].copy_from_slice(&[7,0xa2,5,0,1,0,0,0,0,7,0,5]);
    assert_eq!(thermal::down_clock(&rom),Ok(90));
    rom[0x102]=2;assert!(thermal::down_clock(&rom).is_err());
    rom[0x102]=3;rom[0x103]=255;assert!(thermal::down_clock(&rom).is_err());
}

#[test] fn fuse_access_need_not_echo_and_status_snapshot_is_read_only() {
    let mut io=Io::new();io.no_echo=true;
    thermal::setup(&mut io).unwrap();
    assert_eq!(io.writes[1].readback,0xc0040000);assert_eq!(io.notes,[Ok(1)]);
    assert_eq!(io.writes.len(),6);assert_eq!(io.r[&0x22400],0);
    assert_eq!(thermal::temperature(&mut io),Ok(47));
    let addresses=[0x20000,0x20004,0x20008,0x2000c,0x20044,0x20048,0x20100,
        0x20110,0x20114,0x20134,0x20138,0x201e0,0x201e4,0x201e8,0x201ec,
        0x20200,0x20204,0x20208,0x2020c,0x20210,0x20214,0x20218,0x2021c,
        0x20288,0x2028c,0x20414,0x20480,0x20484,0x204c0,0x204c4,0x200];
    for (n,a) in addresses.iter().enumerate() {io.r.insert(*a,0x1000+n as u32);}
    let before=io.r.clone();let writes=io.writes.len();
    let raw=thermal::status(&mut io);
    assert_eq!(raw.iter().map(|p|p.0).collect::<Vec<_>>(),addresses);
    for (n,(_,v)) in raw.iter().enumerate() {assert_eq!(*v,0x1000+n as u32);}
    assert_eq!(io.r,before);assert_eq!(io.writes.len(),writes);
}
