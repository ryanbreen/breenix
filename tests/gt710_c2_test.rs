extern crate alloc;
#[path = "../kernel/src/gpu/gk208/c2.rs"] mod c2;
use c2::{Access, Limits};
use std::collections::BTreeMap;
fn limits() -> Limits {
    Limits { min_vco:1_100_000,max_vco:2_200_000,min_input:25_000,max_input:100_000,
        min_m:17,max_m:32,min_n:8,max_n:255,min_p:1,max_p:63 }
}
#[test]
fn coefficients_match_original_nouveau_for_both_targets() {
    for reference in [810_000,1_620_000] {
        for requested in [810_000,540_000] {
            let result=std::process::Command::new(std::env::var("GT710_PLL_ORACLE").unwrap())
                .args([reference.to_string(),requested.to_string()]).output().unwrap();
            assert!(result.status.success());
            let text=String::from_utf8(result.stdout).unwrap();
            let oracle:Vec<u32>=text.split_whitespace().map(|s|s.parse().unwrap()).collect();
            let pll=c2::pll(&limits(),reference,requested).unwrap();
            assert_eq!([pll.coefficient,pll.khz],oracle[..]);
            let p=c2::plan("gpc",requested,810_000,&limits(),reference,1_620_000).unwrap();
            assert_eq!((p.khz,p.pll_selected,p.source_divider),(requested,false,if requested==810_000 {2}else{4}));
        }
    }
}
#[derive(Default)]
struct Device { regs:BTreeMap<u32,u32>, writes:Vec<(u32,u32)>, stuck:bool }
impl Access for Device {
    fn read(&mut self,a:u32)->u32 { *self.regs.get(&a).unwrap_or(&0) }
    fn write(&mut self,a:u32,v:u32) {
        assert!(matches!(a,0x137000|0x137004|0x137100|0x137160|0x1371d0));
        self.writes.push((a,v));
        if !self.stuck {self.regs.insert(a,if a==0x137000 {v|0x20000}else{v});}
    }
    fn wait(&mut self,a:u32,mask:u32,value:u32)->Result<(),&'static str> {
        if self.read(a)&mask==value {Ok(())}else{Err("timeout")}
    }
}
#[test]
fn bounded_program_switches_off_pll_then_back_only_when_selected() {
    let mut d=Device::default();d.regs.insert(0x137100,0xfe);
    let p=c2::plan("gpc",540_000,810_000,&limits(),1_620_000,1_620_000).unwrap();
    c2::program(&mut d,&p).unwrap();
    assert_eq!(d.writes.iter().map(|w|w.0).collect::<Vec<_>>(),[0x1371d0,0x137160,0x137100,0x137000,0x137000]);
    assert_eq!(d.read(0x137100),0xfe);assert_eq!(d.read(0x1371d0)&0x8000003f,0x80000004);
    let p=c2::plan("gpc",700_000,810_000,&limits(),1_620_000,1_620_000).unwrap();
    assert!(p.pll_selected);
    d.writes.clear();c2::program(&mut d,&p).unwrap();
    assert_eq!(d.writes[0].0,0x137100);
    assert_eq!(d.writes.last(),Some(&(0x137100,0xff)));
    let coeff=d.writes.iter().position(|w|w.0==0x137004).unwrap();
    assert!(coeff>0 && coeff<d.writes.len()-1);
    let mut d=Device {stuck:true,..Device::default()};d.regs.insert(0x137100,1);
    assert!(c2::program(&mut d,&p).is_err());
    assert_eq!(d.writes.len(),1); // failed source switch must never touch PLL coefficients
}
#[test]
fn post_ceiling_and_other_domains_refused_before_register_writes() {
    for (domain,requested,post) in [("gpc",810_001,810_000),("gpc",810_000,540_000),("gpc",0,810_000),
        ("memory",540_000,810_000),("voltage",540_000,810_000),("sensor",540_000,810_000),("rop",540_000,810_000)] {
        assert!(c2::plan(domain,requested,post,&limits(),1_620_000,1_620_000).is_err());
    }
}
#[test]
fn bounded_v40_limits_decode_and_truncation_refusal() {
    let mut rom=vec![0u8;256];rom[0..6].copy_from_slice(b"\xff\xb8BIT\0");
    rom[8]=12;rom[9]=6;rom[10]=1;rom[11]=0u8.wrapping_sub(rom[..11].iter().fold(0u8,|s,b|s.wrapping_add(*b)));
    rom[12..18].copy_from_slice(&[b'C',1,10,0,32,0]);rom[40..42].copy_from_slice(&64u16.to_le_bytes());
    rom[64..68].copy_from_slice(&[0x40,4,11,1]);rom[68]=8;rom[69..71].copy_from_slice(&96u16.to_le_bytes());
    rom[71..75].copy_from_slice(&0x137000u32.to_le_bytes());
    rom[96..110].copy_from_slice(&[0x4c,4,0x98,8,25,0,100,0,17,32,8,255,1,63]);
    assert_eq!(c2::limits(&rom).unwrap(),limits());
    for n in [0,11,17,41,67,78,109] {assert!(c2::limits(&rom[..n]).is_err());}
}

#[test]
fn snapshot_covers_inactive_paths_and_restore_reinstates_post_pll() {
    use c2::observe::{capture, differences, restore};
    let mut d=Device::default();
    for (a,v) in [(0x137100,0x85),(0x137000,0x30015),(0x137004,0x20412),
                  (0x137160,0x103),(0x1371d0,0x80000b02)] {d.regs.insert(a,v);}
    let post=capture(&mut d);
    for a in [0xe800,0xe804,0xe820,0xe824,0x101000,0x132000,0x132004,0x132020,
              0x132024,0x132030,0x137000,0x137004,0x137008,0x137100,0x137320,0x137330,0x1373f4] {
        assert!(post.iter().any(|p|p.0==a),"missing {a:x}");
    }
    for i in 0..16 {for base in [0x137160,0x1371d0,0x137250] {
        assert!(post.iter().any(|p|p.0==base+i*4));
    }}
    for i in [0,1,2,7] {for base in [0x137000,0x137004,0x137008] {
        assert!(post.iter().any(|p|p.0==base+i*0x20));
    }}
    for i in 0..8 {for base in [0x137120,0x137140] {
        assert!(post.iter().any(|p|p.0==base+i*4));
    }}
    let p=c2::plan("gpc",540_000,810_000,&limits(),1_620_000,1_620_000).unwrap();
    c2::program(&mut d,&p).unwrap();d.writes.clear();
    restore(&mut d,&post).unwrap();
    assert_eq!(d.writes[0],(0x137100,0x84));
    let coef=d.writes.iter().position(|p|*p==(0x137004,0x20412)).unwrap();
    let source=d.writes.iter().position(|p|*p==(0x137100,0x85)).unwrap();
    assert!(coef>0 && coef<source);
    assert!(d.writes.iter().position(|p|*p==(0x1371d0,0x80000b02)).unwrap()>source,
            "restore inactive alternate path only after PLL is selected");
    assert!(differences(&post,&capture(&mut d)).unwrap().is_empty());
    d.stuck=true;d.regs.insert(0x137100,0x85);d.writes.clear();
    assert!(restore(&mut d,&post).is_err());
    assert_eq!(d.writes,[(0x137100,0x84)],"failed source switch must stop before PLL writes");
    let mut missing=post.clone();missing.retain(|p|p.0!=0x137004);
    d.writes.clear();assert!(restore(&mut d,&missing).is_err());assert!(d.writes.is_empty());
}

struct Counter { regs:BTreeMap<u32,u32>, writes:Vec<(u32,u32)>, ns:u64, khz:u32, stalled:bool, inaccessible:bool, records:Vec<(String,Vec<(u32,u32)>)> }
impl Access for Counter {
    fn read(&mut self,a:u32)->u32 {
        self.ns+=10_000;
        if matches!(a,0x18009c|0x180044|0x1800a8)
            && (self.inaccessible || self.regs.get(&0x200).copied().unwrap_or(0)&0x10000000==0) {
            return 0xbadf1201;
        }
        match a {
            0x9400=>if self.stalled {0}else{self.ns as u32},
            0x9410=>if self.stalled {0}else{(self.ns>>32) as u32},
            0x1800a8 if self.regs[&0x18009c]&7==2=>
                (u64::from(*self.regs.get(&a).unwrap_or(&0)) + self.ns*u64::from(self.khz)/1_000_000) as u32,
            _=>*self.regs.get(&a).unwrap_or(&0),
        }
    }
    fn write(&mut self,a:u32,v:u32) {
        assert!(matches!(a,0x200|0x18009c|0x180044|0x1800a8));
        if a==0x200 {
            assert_eq!((self.regs[&a]^v)&!0x10000000,0,"only the PM bit may change");
        } else {
            assert_ne!(self.regs[&0x200]&0x10000000,0,"enable PM before touching counters");
            assert!(!self.inaccessible,"never write a no-PRI-response counter");
        }
        self.writes.push((a,v));self.regs.insert(a,v);
    }
    fn wait(&mut self,_:u32,_:u32,_:u32)->Result<(),&'static str> {unreachable!()}
}
impl c2::observe::CounterAccess for Counter {
    fn monotonic_ns(&mut self)->u64 {self.ns}
    fn record(&mut self,stage:&str,raw:&[(u32,u32)]) {self.records.push((stage.into(),raw.to_vec()));}
}
struct ScopedCounter {
    inner: Counter,
    sampling: bool,
    scoped: bool,
    endpoints: Vec<Vec<u32>>,
}
impl Access for ScopedCounter {
    fn read(&mut self, a:u32)->u32 {
        if self.sampling {
            assert!(self.scoped, "sampling read outside endpoint scope");
            self.endpoints.last_mut().unwrap().push(a);
        }
        self.inner.read(a)
    }
    fn write(&mut self,a:u32,v:u32) {
        assert!(!self.scoped, "write inside endpoint scope");
        self.sampling=false;
        self.inner.write(a,v);
    }
    fn wait(&mut self,a:u32,m:u32,v:u32)->Result<(),&'static str> {
        assert!(!self.scoped, "wait inside endpoint scope");
        self.inner.wait(a,m,v)
    }
}
impl c2::observe::CounterAccess for ScopedCounter {
    fn monotonic_ns(&mut self)->u64 {
        assert!(!self.scoped, "time polling inside endpoint scope");
        self.inner.monotonic_ns()
    }
    fn record(&mut self,stage:&str,raw:&[(u32,u32)]) {
        assert!(!self.scoped, "record inside endpoint scope");
        self.inner.record(stage,raw);
        if stage=="configured" {self.sampling=true;}
    }
    fn endpoint_scope<R>(&mut self, f:impl FnOnce(&mut Self)->R)->R {
        assert!(!self.scoped);
        self.scoped=true;
        self.endpoints.push(Vec::new());
        let result=f(self);
        self.scoped=false;
        result
    }
}
#[test]
fn each_counter_point_has_its_own_read_only_endpoint_scope() {
    let mut d=ScopedCounter {inner:bracket_replay().inner,sampling:false,scoped:false,endpoints:Vec::new()};
    c2::observe::measure(&mut d).unwrap();
    assert!(d.endpoints.len()>=2);
    for endpoint in &d.endpoints {
        assert_eq!(endpoint, &[0x9410,0x9400,0x9410,0x1800a8,0x9410,0x9400,0x9410]);
    }
}
// Model the observed target clock and a delayed final PTIMER read. The record
// reports the rejection, while the endpoint stall is the replay hypothesis.
struct BracketReplay { inner: Counter, sampling: bool, first: Option<u64>, stalls: u32, max_stalls: u32, delay: u64 }
impl Access for BracketReplay {
    fn read(&mut self, a:u32)->u32 {
        self.inner.ns=self.inner.ns.saturating_sub(8_000); // 2 us per register read
        let v=self.inner.read(a);
        if self.sampling && a==0x1800a8 {
            let first=*self.first.get_or_insert(self.inner.ns);
            if self.inner.ns-first>=20_000_000 && self.stalls<self.max_stalls {
                self.inner.ns+=self.delay;
                self.stalls+=1;
                self.first=None;
            }
        }
        v
    }
    fn write(&mut self,a:u32,v:u32) {self.inner.write(a,v)}
    fn wait(&mut self,a:u32,m:u32,v:u32)->Result<(),&'static str> {self.inner.wait(a,m,v)}
}
impl c2::observe::CounterAccess for BracketReplay {
    fn monotonic_ns(&mut self)->u64 {self.inner.ns}
    fn record(&mut self,stage:&str,raw:&[(u32,u32)]) {
        self.inner.record(stage,raw);
        if stage=="configured" {self.sampling=true;}
        if stage=="restore-halted" {self.sampling=false;}
    }
}
fn bracket_replay()->BracketReplay {
    BracketReplay {inner:Counter {regs:BTreeMap::from([(0x200,0x40501100),(0x18009c,0),
        (0x180044,0x1234),(0x1800a8,99)]),writes:Vec::new(),ns:4_290_000_000,
        khz:81_653,stalled:false,inaccessible:false,records:Vec::new()},
        sampling:false,first:None,stalls:0,max_stalls:1,delay:2_900_000}
}
#[test]
fn delayed_endpoint_retries_with_a_fresh_verified_window() {
    let mut d=bracket_replay();
    let original=d.inner.regs.clone();
    let m=c2::observe::measure(&mut d).unwrap();
    assert_eq!(d.stalls,1);
    assert!(m.khz.abs_diff(81_653)<100);
    assert!(m.ptimer_ns>=20_000_000 && m.bracket_ns<=m.ptimer_ns/100);
    assert_eq!(d.inner.regs,original);
}
#[test]
fn repeated_wide_endpoints_remain_failures() {
    for delay in [2_900_000,185_000] {
        let mut d=bracket_replay();
        d.max_stalls=3;
        d.delay=delay;
        let original=d.inner.regs.clone();
        assert_eq!(c2::observe::measure(&mut d).err(),Some("counter/PTIMER bracket exceeds 1 percent"));
        assert_eq!(d.stalls,3);
        assert_eq!(d.inner.regs,original);
    }
}

#[test]
fn independent_counter_tracks_cycles_across_ptimer_and_restores_on_timeout() {
    use c2::observe::measure;
    for khz in [540_000,810_000] {
        let original=BTreeMap::from([(0x200,0x40501100),(0x18009c,0),(0x180044,0x1234),(0x1800a8,99)]);
        let mut d=Counter {regs:original.clone(),writes:Vec::new(),ns:4_290_000_000,khz,stalled:false,inaccessible:false,records:Vec::new()};
        let m=measure(&mut d).unwrap();
        assert!(m.khz.abs_diff(khz)<1000,"{} versus {khz}",m.khz);
        assert!(m.ptimer_ns>=20_000_000);assert!(m.cycles>0);
        assert!(m.bracket_ns<=m.ptimer_ns/100);
        assert_eq!(d.regs,original);
        d.stalled=true;d.writes.clear();assert!(measure(&mut d).is_err());assert_eq!(d.regs,original);
        // C2c explicitly authorizes restored setup of the inherited active state.
        d.stalled=false;d.regs.insert(0x18009c,0x40002);d.writes.clear();d.records.clear();
        let saved=d.regs.clone();
        let m=measure(&mut d).unwrap();
        assert!(m.khz.abs_diff(khz)<1000);
        assert_eq!(d.regs[&0x18009c],saved[&0x18009c]);
        assert_eq!(d.regs[&0x180044],saved[&0x180044]);
        assert_eq!(d.writes.iter().find(|w|w.0!=0x200),Some(&(0x18009c,0)),"halt inherited mode before changing PRE");
        let restored=d.records.iter().find(|r|r.0=="restore-halted").unwrap();
        let saved_record=d.records.iter().find(|r|r.0=="saved").unwrap();
        let saved_count=saved_record.1.iter().find(|r|r.0==0x1800a8).unwrap().1;
        assert!(restored.1.contains(&(0x1800a8,saved_count)));
        assert_eq!(d.writes.iter().rfind(|w|w.0!=0x200),Some(&(0x18009c,0x40002)));
        assert!(d.records.iter().any(|r|r.0=="restored" && r.1.contains(&(0x18009c,0x40002))));
        d.stalled=true;d.writes.clear();
        assert!(measure(&mut d).is_err());
        assert_eq!(d.regs[&0x18009c],0x40002);
        assert_eq!(d.regs[&0x180044],saved[&0x180044]);
    }
}

#[test]
fn pm_is_enabled_before_sampling_and_restored_after_counter_cleanup() {
    for (pm,stalled,inaccessible) in [(0x40501100,false,false),(0x50501100,false,false),
                                      (0x40501100,true,false),(0x40501100,false,true)] {
        let original=BTreeMap::from([(0x200,pm),(0x18009c,0),(0x180044,0x1234),(0x1800a8,99)]);
        let mut d=Counter {regs:original.clone(),writes:Vec::new(),ns:0,khz:810_000,
            stalled,inaccessible,records:Vec::new()};
        let result=c2::observe::measure(&mut d);
        if stalled || inaccessible {assert!(result.is_err());}else{
            assert!(result.as_ref().unwrap().khz.abs_diff(810_000)<1000);
        }
        assert_eq!(d.regs,original,"restore all touched state even on sampling failure");
        assert_eq!(d.writes.first(),Some(&(0x200,pm|0x10000000)));
        assert_eq!(d.writes.last(),Some(&(0x200,pm)));
        assert!(d.records.iter().any(|r|r.0=="pm-before" && r.1.contains(&(0x200,pm))));
        assert!(d.records.iter().any(|r|r.0=="pm-enabled" && r.1.contains(&(0x200,pm|0x10000000))));
        assert!(d.records.iter().any(|r|r.0=="pm-restored" && r.1.contains(&(0x200,pm))));
        if inaccessible {
            assert_eq!(result.err(),Some("GPC counter has no PRI response after PM enable"));
            assert_eq!(d.writes.len(),2,"unresponsive counters must not be written");
        }
    }
}

#[test]
fn c3_pll_coefficients_and_lock_before_source_at_the_three_authorized_targets() {
    for requested in [810_000,1_080_000,1_307_000] {
        let oracle=std::process::Command::new(std::env::var("GT710_PLL_ORACLE").unwrap())
            .args([810_000.to_string(),requested.to_string()]).output().unwrap();
        assert!(oracle.status.success());
        let expected:Vec<u32>=String::from_utf8(oracle.stdout).unwrap().split_whitespace().map(|s|s.parse().unwrap()).collect();
        let p=c2::pll_plan("gpc",requested,&limits(),810_000).unwrap();
        assert!(p.pll_selected);assert_eq!([p.coefficient,p.khz],expected[..]);
        struct Sequence { regs:BTreeMap<u32,u32>, events:Vec<(u32,u32,u32)>, lock:bool }
        impl Access for Sequence {
            fn read(&mut self,a:u32)->u32 {*self.regs.get(&a).unwrap_or(&0)}
            fn write(&mut self,a:u32,v:u32) {
                assert!(matches!(a,0x137100|0x137000|0x137004));
                self.events.push((a,0,v));self.regs.insert(a,v);
            }
            fn wait(&mut self,a:u32,m:u32,v:u32)->Result<(),&'static str> {
                self.events.push((a,m,v));
                if a==0x137000 && self.lock {let c=self.read(a);self.regs.insert(a,c|m);}
                if self.read(a)&m==v {Ok(())}else{Err("lock timeout")}
            }
        }
        for lock in [true,false] {
            let mut d=Sequence {regs:BTreeMap::from([(0x137000,0x10000),(0x137100,0)]),events:Vec::new(),lock};
            let result=c2::program(&mut d,&p);
            assert_eq!(result.is_ok(),lock);
            assert_eq!(&d.events[..8],&[(0x137100,0,0),(0x137100,1,0),
                (0x137000,0,0x10000),(0x137000,0,0x10000),
                (0x137004,0,p.coefficient),(0x137000,0,0x10001),
                (0x137000,0,0x10001),(0x137000,0x20000,0x20000)]);
            if lock {assert_eq!(&d.events[8..],&[(0x137000,0,0x30011),(0x137000,0,0x30015),
                (0x137100,0,1),(0x137100,1,1)]);}else{
                assert_eq!(d.events.len(),8,"failed lock must never select the PLL");
                assert_eq!(d.read(0x137100),0);
            }
        }
    }
    for (domain,khz) in [("gpc",1_307_001),("gpc",0),("memory",1_080_000),("voltage",1_080_000)] {
        assert!(c2::pll_plan(domain,khz,&limits(),810_000).is_err());
    }
}
