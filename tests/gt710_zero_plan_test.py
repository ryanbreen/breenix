"""Native image zero initialization uses constant CE work without backing mappings."""
from pathlib import Path
import unittest
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class ZeroPlan(unittest.TestCase):
 def test_constant_zero_exact_extent_and_ordered_engine_completion(self):
  source=(ROOT/'kernel/src/gpu/gk208/copy.rs').read_text().replace('//!','//').replace('use alloc::{vec, vec::Vec};','use std::vec::Vec;')
  if 'pub fn zero(' not in source:
   source+="\nfn zero(i:Image)->Result<Plan,&'static str>{upload(i,Linear{address:0x4000000,bytes:u64::from(i.pitch)*u64::from(i.height),offset:0,stride:i.pitch},[0,0,i.width,i.height])}\n"
  run_rust(self,source+r'''
#[cfg(test)]mod zero_check{use super::*;use std::collections::BTreeMap;
fn regs(words:&[u32])->(BTreeMap<u32,u32>,Vec<(u32,BTreeMap<u32,u32>)>){let(mut at,mut r,mut launches)=(0,BTreeMap::new(),vec![]);while at<words.len(){let h=words[at];let address=(h&0x1fff)*4;let count=((h>>16)&0x1fff)as usize;for j in 0..count{r.insert(address+j as u32*4,words[at+1+j]);}if address==0x300{launches.push((r[&0x300],r.clone()));}at+=count+1;}(r,launches)}
#[test]fn zero_extent_and_completion(){for (bpp,w,h) in [(4u32,2560u32,1600u32),(4,3,4),(1,17,9)]{let pitch=(w*bpp).next_multiple_of(64);let image=Image{address:0x200000,width:w,height:h,texel_bytes:bpp,pitch,block_height:1,bytes:u64::from(pitch)*u64::from(h.next_multiple_of(8))};let(_,launches)=regs(&encode_plan(zero(image).unwrap(),0x10000,7,19).unwrap());assert_eq!(launches.len(),4);let(launch,r)=&launches[0];assert_eq!(*launch,0x686);assert_eq!(r[&0x700],0);assert_eq!(r[&0x708],4|((bpp-1)<<16));assert_eq!((r[&0x408],r[&0x40c]),(0,image.address as u32));assert_eq!((r[&0x418],r[&0x41c]),(w,h));assert_eq!((r[&0x710],r[&0x714]),(w,h));assert_eq!(r[&0x720],0);for (i,(launch,r)) in launches[1..].iter().enumerate(){assert_eq!(*launch,0xc);assert_eq!(r[&0x244],0x10000+[4,8,0][i]);assert_eq!(r[&0x248],[7,0,19][i]);}
let(_,skipped)=regs(&encode_plan(zero(image).unwrap().with_extras(Extras{skip:true,stale_fence:false}),0x10000,7,19).unwrap());assert_eq!(skipped.len(),3);}
assert!(zero(Image{address:1,width:1,height:1,texel_bytes:4,pitch:64,block_height:1,bytes:512}).is_err());}
}
''')
if __name__=='__main__':unittest.main()
