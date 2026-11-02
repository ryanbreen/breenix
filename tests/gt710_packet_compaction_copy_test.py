"""Copy retained native command spans rather than individual method packets."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class PacketCopies(unittest.TestCase):
    def test_copy_calls_are_bounded_by_removed_spans(self):
        encoder = (ROOT / 'kernel/src/gpu/gk208/encode.rs').read_text().replace(
            'include_bytes!("caps/integer.inputs")',
            'include_bytes!("' + str(ROOT / 'kernel/src/gpu/gk208/caps/integer.inputs') + '")')
        encoder = re.sub(r'words\.copy_within\((.*?)\);', r'crate::copy_words(words, \1);', encoder)
        source = r'''
extern crate alloc;
use std::{ops::Range,sync::atomic::{AtomicUsize,Ordering}};
static COPIES:AtomicUsize=AtomicUsize::new(0);
fn copy_words(w:&mut Vec<u32>,r:Range<usize>,to:usize){COPIES.fetch_add(1,Ordering::Relaxed);w.copy_within(r,to);}
#[allow(dead_code)]#[path="SHADERS"]mod shaders;
#[allow(dead_code)]#[path="CAPS"]mod caps;
#[allow(dead_code)]mod encode;
fn emit(w:&mut Vec<u32>,m:u32,d:&[u32]){w.push(0x20000000|((d.len()as u32)<<16)|m/4);w.extend(d);}
fn packets(w:&[u32])->usize{let(mut at,mut n)=(0,0);while at<w.len(){at+=1+((w[at]>>16)&0x1fff)as usize;n+=1;}assert_eq!(at,w.len());n}
fn main(){
 let mut words=Vec::new();emit(&mut words,0,&[0xa197]);
 for n in 0..8{
  emit(&mut words,0x800,&[0,0x400000,256,256,0xcf,0x30,1,0,0]);
  emit(&mut words,0xa00,&[128f32.to_bits(),128f32.to_bits(),1f32.to_bits(),128f32.to_bits(),128f32.to_bits(),0]);
  emit(&mut words,0xc00,&[256<<16,256<<16,0,1f32.to_bits()]);emit(&mut words,0xe00,&[1,256<<16,256<<16]);
  emit(&mut words,0x1608,&[0,0x800000]);emit(&mut words,0x2040,&[0x11,0x30]);emit(&mut words,0x204c,&[4]);
  emit(&mut words,0x2140,&[0x51,if n<4{0x1b0}else{0x2b0}]);emit(&mut words,0x214c,&[4]);
  for k in 0..60{emit(&mut words,0x121c,&[k]);}
  emit(&mut words,0xda4,&[0x1011]);emit(&mut words,0xda4,&[1]);
  emit(&mut words,0x2490,&[1]);emit(&mut words,0x1618,&[4]);emit(&mut words,0xd74,&[0,6]);emit(&mut words,0x1614,&[0]);
 }
 let mut compacted=words.clone();let before=packets(&compacted);COPIES.store(0,Ordering::Relaxed);
 encode::compact_state(&mut compacted).unwrap();let removed=before-packets(&compacted);
 assert!(removed>40);let copies=COPIES.load(Ordering::Relaxed);
 assert!(copies<=removed+1,"state filter copies {copies} times for {removed} removed packets; copy kept spans once");
 let before=packets(&words);COPIES.store(0,Ordering::Relaxed);encode::retain_resident_instruction_cache(&mut words).unwrap();
 let removed=before-packets(&words);assert_eq!(removed,8);let copies=COPIES.load(Ordering::Relaxed);
 assert!(copies<=removed+1,"cache filter copies {copies} times for {removed} removed packets; copy kept spans once");
}
'''.replace('SHADERS', str(ROOT / 'kernel/src/gpu/gk208/gr/shaders.rs')).replace('CAPS', str(ROOT / 'kernel/src/gpu/gk208/caps.rs'))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-packet-copies-') as directory:
            p = Path(directory)
            (p / 'encode.rs').write_text(encoder)
            (p / 'check.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi', '--crate-type', 'rlib', '--cfg', 'feature="alloc"', '-Dwarnings', str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(p / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--extern', f'gpu_abi={p / "libgpu_abi.rlib"}', '-Dwarnings', str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
