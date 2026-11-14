"""Catalogue boundaries that must not disable a working native console."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class CatalogueEdges(unittest.TestCase):
    def test_single_timing_and_wire_capacity(self):
        code = r'''#![allow(dead_code)] // Include production display encoders on the host.
extern crate alloc;
#[path="ROOT/kernel/src/gpu/gk208/edid.rs"] mod edid;
#[path="ROOT/kernel/src/gpu/gk208/modeset.rs"] mod modeset;
fn checksum(e: &mut [u8]) {
 for b in e.chunks_exact_mut(128) { b[127]=0; b[127]=0u8.wrapping_sub(b.iter().fold(0u8,|a,v|a.wrapping_add(*v))); }
}
fn dtd(w:u32,h:u32) -> [u8;18] {
 let mut d=[0u8;18]; d[..2].copy_from_slice(&6500u16.to_le_bytes());
 d[2]=w as u8;d[3]=160;d[4]=((w>>8)<<4) as u8;
 d[5]=h as u8;d[6]=40;d[7]=((h>>8)<<4) as u8;
 d[8]=24;d[9]=80;d[10]=0x36;d[17]=24; d
}
fn main() {
 let mut e=vec![0u8;128]; e[..8].copy_from_slice(&[0,255,255,255,255,255,255,0]);
 e[18]=1;e[19]=4;e[20]=128;e[54..72].copy_from_slice(&dtd(1024,768));checksum(&mut e);
 let one=modeset::display_modes(&e).expect("a preferred-only monitor must boot");
 assert_eq!(one,[(2048,1536,2),(1024,768,1)]);
 e.resize(512,0);e[126]=3;
 for n in 1..=3 {let at=n*128;e[at]=2;e[at+2]=4;
  for j in 0..6 {let w=640+(n*6+j) as u32*32;let h=480+(n*6+j) as u32*8;
   e[at+4+j*18..at+22+j*18].copy_from_slice(&dtd(w,h));}}
 checksum(&mut e);
 let many=modeset::display_modes(&e).unwrap();assert_eq!(many.len(),10);
 assert_eq!(many[1],(1024,768,1));assert_eq!(many.last(),Some(&(2048,1536,2)));
 assert_eq!(many.iter().filter(|m|m.2==2).count(),1);
}
'''.replace('ROOT', str(ROOT))
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'check.rs').write_text(code)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)

if __name__=='__main__': unittest.main()
