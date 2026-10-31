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
