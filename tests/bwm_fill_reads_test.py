"""Execute the six startup samples with bounded readback round trips."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class FillReads(unittest.TestCase):
 def test_samples_and_binding_with_bounded_transfers(self):
  src=(ROOT/'userspace/programs/src/bwm/compositor.rs').read_text()
  run_rust(self,r'''
use std::sync::Mutex;
static READS:Mutex<Vec<[u32;4]>>=Mutex::new(Vec::new());
static BAD:Mutex<Option<(u32,u32)>>=Mutex::new(None);
static FLAGS:Mutex<u32>=Mutex::new(1);
static CHANGED:Mutex<bool>=Mutex::new(false);
const CHECK_BAND_BYTES:u32=4*1024*1024;
mod abi{pub mod record{pub const SIZE:usize=1024;pub const FLAGS:usize=24;pub const TARGET_WIDTH:usize=48;pub const TARGET_HEIGHT:usize=52;pub const RESOURCE:usize=80;}}
fn prefaulted_bytes(n:usize)->Vec<u8>{vec![0;n]}
@RESIZE@
fn pixel(x:u32,y:u32)->u32{0xff000000|((x*3+y*257)&0xffffff)}
mod gpu{
 pub struct Answer{pub reason:u32}
 pub fn readback_why(_:u64,r:[u32;4],out:&mut[u8],stride:u32,rec:&mut[u8;1024])->(i64,Answer){
  let mut reads=super::READS.lock().unwrap();reads.push(r);
  assert!(r[2]*r[3]*4<=super::CHECK_BAND_BYTES);
  assert_eq!(stride,r[2]*4);
  for y in 0..r[3]{for x in 0..r[2]{let p=(r[0]+x,r[1]+y);let mut c=super::pixel(p.0,p.1);if *super::BAD.lock().unwrap()==Some(p){c^=1;}let at=((y*r[2]+x)*4)as usize;out[at..at+4].copy_from_slice(&c.to_le_bytes());}}
  for (at,v) in [(24,*super::FLAGS.lock().unwrap()),(48,2560),(52,1600),(80,7+u32::from(*super::CHANGED.lock().unwrap()&&reads.len()>1))]{rec[at..at+4].copy_from_slice(&v.to_le_bytes());}
  (0,Answer{reason:0})
 }
}
@SCANOUT@
struct Compositor{width:u32,height:u32}
impl Compositor{@CHECK@}
#[test]fn six_samples_keep_pixels_and_binding(){
 let c=Compositor{width:2560,height:1600};
 let points=[(0,0),(2559,55),(2559,56),(2559,1527),(0,1528),(2559,1599)];
 let samples:Vec<_>=points.iter().map(|&(x,y)|(x,y,pixel(x,y))).collect();
 let seen=c.check_fills(&samples).unwrap();assert_eq!((seen.resource,seen.width,seen.height),(7,2560,1600));
 assert!(READS.lock().unwrap().len()<=2,"six fill samples serialized six GPU round trips");
 for p in points{READS.lock().unwrap().clear();*BAD.lock().unwrap()=Some(p);assert!(c.check_fills(&samples).is_err(),"missed sample {p:?}");}
 *BAD.lock().unwrap()=None;READS.lock().unwrap().clear();*FLAGS.lock().unwrap()=0;assert!(c.check_fills(&samples).is_err(),"accepted missing scanout proof");
 *FLAGS.lock().unwrap()=1;READS.lock().unwrap().clear();*CHANGED.lock().unwrap()=true;assert!(c.check_fills(&samples).is_err(),"accepted changed binding");
 assert!(c.check_fills(&[]).is_err());
}
'''.replace('@CHECK@',item(src,'pub fn check_fills(')).replace('@SCANOUT@',item(src,'pub struct Scanout {')).replace('@RESIZE@',item(src,'fn resize_readback(') if 'resize_readback(' in item(src,'pub fn check_fills(') else ''))
if __name__=='__main__':unittest.main()
