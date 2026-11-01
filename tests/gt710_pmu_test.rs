#[path="../kernel/src/gpu/gk208/pmu.rs"]
mod pmu;
use std::collections::BTreeMap;
#[derive(Default)]
struct Io { regs:BTreeMap<u32,u32>, writes:Vec<(u32,u32)>, waits:Vec<(&'static str,u32,u64,bool)>, us:u64, send_at:u64, recv_at:u64 }
impl pmu::Access for Io {
    fn read(&mut self,a:u32)->u32 {
        match a {
            0x10a4d0 if self.us>=self.send_at=>0x00010000,
            0x10a4dc if self.us>=self.recv_at=>0x00000001,
            _=>*self.regs.get(&a).unwrap_or(&0),
        }
    }
    fn write(&mut self,a:u32,v:u32) {self.writes.push((a,v));self.regs.insert(a,v);}
    fn now_us(&mut self)->u64 {self.us+=1000;self.us}
    fn waited(&mut self,name:&'static str,raw:u32,us:u64,ok:bool) {self.waits.push((name,raw,us,ok));}
}
fn images()->(Vec<u8>,Vec<u8>) {((0u32..128).flat_map(u32::to_le_bytes).collect(),vec![1,2,3,4,5,6,7,8])}
#[test]
fn reset_upload_start_and_only_nouveau_ring_success_conditions() {
    let mut io=Io {send_at:10000,recv_at:20000,..Io::default()};io.regs.insert(0x200,0xe031316d);
    let (code,data)=images();let r=pmu::start(&mut io,&code,&data).unwrap();
    assert_eq!((r.send,r.recv),(0x10000,1)); // No added size/base validation.
    let mut expected=vec![(0x10a014,0xffff),(0x200,0xe031116d),(0x200,0xe031316d),(0x10a1c0,0x1000000),(0x10a1c4,0x04030201),(0x10a1c4,0x08070605),(0x10a180,0x1000000)];
    for i in 0..128 {if i%64==0 {expected.push((0x10a188,i/64));}expected.push((0x10a184,i));}
    expected.extend([(0x10a10c,0),(0x10a104,0),(0x10a100,2),(0x10a010,0xe0)]);
    assert_eq!(io.writes,expected); // No ring writes, commands, or INIT packet.
    assert!(io.waits.iter().all(|r|r.3));
    assert_eq!(io.waits.iter().map(|r|r.0).collect::<Vec<_>>(),["idle","scrub","send","recv"]);
}
#[test]
fn each_ring_has_its_own_two_second_budget_and_first_failure_stops() {
    let (code,data)=images();
    for (send,recv,name) in [(u64::MAX,0,"send"),(1_900_000,u64::MAX,"recv")] {
        let mut io=Io {send_at:send,recv_at:recv,..Io::default()};
        assert!(pmu::start(&mut io,&code,&data).is_err());
        let row=io.waits.last().unwrap();assert_eq!(row.0,name);assert!(!row.3);assert_eq!(row.2,2_000_000);
        assert!(!io.writes.contains(&(0x10a010,0xe0)));
    }
    let mut io=Io {send_at:1_900_000,recv_at:3_800_000,..Io::default()};
    assert!(pmu::start(&mut io,&code,&data).is_ok());
    assert!(io.waits.iter().filter(|r|r.0=="send" || r.0=="recv").all(|r|r.2<2_000_000));
}
#[test]
fn idle_and_scrub_timeouts_are_observations_as_in_nouveau() {
    let (code,data)=images();let mut io=Io::default();
    io.regs.extend([(0x200,0x2000),(0x10a04c,1),(0x10a10c,6)]);
    assert!(pmu::start(&mut io,&code,&data).is_ok());
    assert!(!io.waits[0].3 && !io.waits[1].3);
}
