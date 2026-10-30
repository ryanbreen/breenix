#![allow(dead_code)] // Real present.rs also includes command encoders outside this fixture.
extern crate alloc;
extern crate self as spin;
use std::collections::BTreeMap;
pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {pub const fn new(v:T)->Self{Self(std::sync::Mutex::new(v))}pub fn lock(&self)->std::sync::MutexGuard<'_,T>{self.0.lock().unwrap()}}
struct Registers(BTreeMap<usize,u32>);
impl Registers {fn read(&self,r:usize)->u32{self.0[&r]}}
#[path="@ROOT@/kernel/src/gpu/gk208/present.rs"] mod present;
#[path="@ROOT@/kernel/src/gpu/gk208/display_evidence.rs"] mod display_evidence;
mod block {pub mod install {#[path="@ROOT@/libs/libbreenix/src/crypto/sha256.rs"] pub mod sha256;}}
mod firmware_writes {pub fn count()->u64{51}}
fn hex(b:&[u8])->String{b.iter().map(|v|format!("{v:02x}")).collect()}
#[test] fn retain_latest_two_modes_and_export_raw_registers_with_matching_digests(){
 display_evidence::baseline(50);
 let mut regs=Registers(BTreeMap::new());
 for bank in [0x640000,0x660000]{for m in [0x42c,0x494,0x4b0,0x4b8,0x4bc,0x4c0,0x4c4,0x4c8]{regs.0.insert(bank+m,m as u32);}}
 regs.0.insert(0x64041c,123);regs.0.insert(0x640420,456);
 for n in 0..500 {
  let (w,h)=if n%2==0{(1280,800)}else{(2560,1600)};
  let surface=present::Surface{resource:n+1,address:0x1000000,width:w,height:h,pitch:w*4,block_height:0,format:0,dma:1};
  // Keep the latch distinct from the requested surface in this transport
  // check: the observer must report the raw receipt, not reconstruct it.
  let receipt=present::Receipt{notifier:1,active:[0x20000,2,3,4,5],armed:[0x20000,2,3,4,5]};
  display_evidence::record(&mut regs,0,surface,&receipt);
 }
 display_evidence::sample(0,64,0x12345678,0xdeadbeef);
 display_evidence::sample(0,128,0,0); // Preserve the first mismatch.
 display_evidence::sample(1,256,0xaabbccdd,0x11223344);
 let exported=display_evidence::export();assert_eq!(exported.len(),4);
 assert!(exported[2].0.contains("\"stage\":0,\"offset\":64,\"expected\":305419896,\"actual\":3735928559"));
 assert!(exported[3].0.contains("\"stage\":1,\"offset\":256,\"expected\":2864434397,\"actual\":287454020"));
 for ((text,anchor),seq) in exported.iter().zip([499,500]) {
  assert!(text.contains(&format!("\"sequence\":{seq}")));
  assert!(text.contains("\"requested\":16777216"));assert!(text.contains("\"active\":[131072, 2, 3, 4, 5]"));
  assert!(text.contains("\"viewport_active\":[1068, 1172, 1200, 1208, 1212, 1216, 1220, 1224]"));
  assert!(text.contains("\"raster\":[123, 456]"));assert!(text.contains("\"firmware_writes\":1"));
  assert_eq!(anchor,&format!("GT710_DISPLAY_BULK sequence={seq} bytes={} sha256={}",text.len(),hex(&block::install::sha256::sha256(text.as_bytes()))));
 }
 assert!(display_evidence::export().is_empty());
}
