//! C5c tests use synthetic ROM tables and MMIO; no board ROM is redistributed.
extern crate alloc;
// The host fixture links the display parser; unrelated display entry points are unused.
#[allow(dead_code)]
#[path="../kernel/src/gpu/gk208/edid.rs"] mod edid;
#[allow(dead_code)]
#[path="../kernel/src/gpu/gk208/modeset.rs"] mod modeset;
#[path="../kernel/src/gpu/gk208/ram_init.rs"] mod ram_init;
use std::collections::BTreeMap;
struct Io { regs:BTreeMap<usize,u32>, writes:Vec<(usize,u32)> }
impl modeset::Io for Io {
 fn read(&mut self,a:usize)->u32 {*self.regs.get(&a).unwrap_or(&0)}
 fn write(&mut self,a:usize,v:u32) {self.writes.push((a,v));self.regs.insert(a,v);}
 fn delay_us(&mut self,_:u32) {panic!("RAM scripts have no delay");}
}
fn script(base:u32,incr:u8,values:&[[u32;3]])->Vec<u8> {
 let mut r=vec![0,0x8f];r.extend(base.to_le_bytes());r.extend([incr,values.len() as u8]);
 for group in values {for v in group {r.extend(v.to_le_bytes());}}
 r.push(0x71);r
}
#[test]
fn ram_restricted_group_consumes_all_groups_and_increments_once_per_register() {
 let r=script(0x11d6a0,8,&[[11,12,13],[21,22,23]]);
 let ops=modeset::ram_script(&r,1,3,1).unwrap();
 let mut io=Io {regs:BTreeMap::new(),writes:vec![]};modeset::execute_ops(&mut io,&ops);
 assert_eq!(io.writes,vec![(0x11d6a0,12),(0x11d6a8,22)]);
 // Nouveau init.c:1995-2008 consumes unmatched groups without an MMIO write.
 let ops=modeset::ram_script(&r,1,3,7).unwrap();io.writes.clear();
 modeset::execute_ops(&mut io,&ops);assert!(io.writes.is_empty());
 // The display route must retain its register boundary.
 assert!(modeset::script(&r,1,0,0,1).is_err());
 assert!(modeset::ram_script(&r[..r.len()-2],1,3,1).is_err());
}
#[test]
fn init_skips_current_mode_restores_only_selector_and_keeps_fixed_write_order() {
 let scripts:Vec<_>=(0..3).map(|i|modeset::ram_script(&script(0x11d67c,4,&[[10+i,20+i,30+i]]),1,3,1).unwrap()).collect();
 for current in 0..3 {
  let saved=0xa5000003 | current<<4;
  let mut io=Io {regs:BTreeMap::from([(0x10f65c,saved),(0x10f584,0xffffffff),(0x10f160,0x12340000)]),writes:vec![]};
  ram_init::execute(&mut io,&scripts);
  let mut expected=vec![];
  for i in 0..3 {if i!=current {expected.push((0x10f65c,(saved&!0xf0)|(i<<4)));expected.push((0x11d67c,20+i));}}
  expected.extend([(0x10f65c,saved),(0x10f584,0xeeffffff),(0x10ecc0,0xffffffff),(0x10f160,0x12340010)]);
  assert_eq!(io.writes,expected);
 }
}
