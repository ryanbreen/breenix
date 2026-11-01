"""Resident batches preserve cache masks without repeated implicit idle waits."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ResidentCache(unittest.TestCase):
    def test_resident_and_parked_cache_packets_keep_their_bits(self):
        encoder = (ROOT / 'kernel/src/gpu/gk208/encode.rs').read_text().replace(
            'include_bytes!("caps/integer.inputs")',
            'include_bytes!("' + str(ROOT / 'kernel/src/gpu/gk208/caps/integer.inputs') + '")')
        check = r'''
pub fn check() {
 fn caches(words:&[u32])->Vec<(u32,u32)> {
  let mut out=Vec::new();let mut at=0;
  while at<words.len(){let n=((words[at]>>16)&0x1fff)as usize;
   let m=(words[at]&0x1fff)*4;if matches!(m,0x21c|0xda4){assert_eq!(n,1);out.push((m,words[at+1]));}at+=n+1;}
  out
 }
 let t=Target{id:1,address:0x400000,width:256,height:256,format:gpu_abi::format::B8G8R8A8,tile_mode:0};
 let mut full=Frame{probe_mask:0,chunks:Vec::new(),data:vec![0;DATA_START],removed_prefix:0};
 let mut resident=Frame{probe_mask:0,chunks:Vec::new(),data:Vec::new(),removed_prefix:DATA_START};
 for f in [&mut full,&mut resident] {
  quads(f,t,[[2,4,18,20],[30,50,32,80]].into_iter(),[0,0,256,256],&[1,2,3,4],gpu_abi::pipeline::SOLID,None,None,None).unwrap();
 }
 assert_eq!(caches(&full.chunks[0].words),[(0x21c,0x1011),(0x21c,1)]);
 assert_eq!(caches(&resident.chunks[0].words),[(0xda4,0x1011),(0xda4,1)],"resident cache operations need no implicit WFI");
 full.compact().unwrap();
 let mut normalized=resident.chunks[0].words.clone();let mut at=0;
 while at<normalized.len(){let n=((normalized[at]>>16)&0x1fff)as usize;
  if normalized[at]&0x1fff==0xda4/4{normalized[at]=(normalized[at]&!0x1fff)|0x21c/4;}at+=n+1;}
 assert_eq!(normalized,full.chunks[0].words,"other state, launches and relocation offsets stay byte-identical");
 assert_eq!(resident.data,full.data);
 let (_,parked)=parking_frame(t,Target{id:2,address:0x500000,..t},0x600000,0x800000).unwrap();
 assert_eq!(caches(&parked),[(0xda4,0x1011),(0xda4,1)]);
 assert_eq!(&parked[..2],&[0x20010044,0]);
 assert_eq!(&parked[parked.len()-2..],&[0x20010044,0]);
}
'''
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-resident-cache-') as directory:
            p = Path(directory)
            (p / 'encode.rs').write_text(encoder + check)
            (p / 'check.rs').write_text('''extern crate alloc;
#[allow(dead_code)] // The complete production catalogue supports the encoder.
#[path="SHADERS"] pub mod shaders;
#[allow(dead_code)] // The production encoder also contains probe geometry.
#[path="CAPS"] pub mod caps;
#[allow(dead_code)] // Exercise the private quad builder in its production module.
mod encode;
fn main(){encode::check();}
'''.replace('SHADERS', str(ROOT / 'kernel/src/gpu/gk208/gr/shaders.rs'))
                .replace('CAPS', str(ROOT / 'kernel/src/gpu/gk208/caps.rs')))
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                           '--crate-type', 'rlib', '--cfg', 'feature="alloc"', '-Dwarnings',
                           str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(p / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--extern',
                           f'gpu_abi={p / "libgpu_abi.rlib"}', str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
