"""Retain allocation-site evidence with the final native upload, off serial."""
from pathlib import Path
import json
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]

class ArtifactMemory(unittest.TestCase):
    def test_final_resource_row_retains_the_actual_frame_census(self):
        source = (ROOT / 'userspace/programs/src/gt710_artifacts.rs').read_text()
        body = source[source.index('    let mut data='):source.index('    if data.is_empty()')]
        fixture = r'''
mod libbreenix { pub mod gpu { pub mod abi {
 #[repr(C)] pub struct Caps {pub bytes: [u8;512]}
}}}
pub mod std {pub mod fs {
 pub fn read(path:&str)->Result<Vec<u8>,()> {
  assert_eq!(path,"/proc/gt710-pixels");Ok(b"startup and runtime probes\n".to_vec())
 }
 pub fn read_to_string(path:&str)->Result<String,()> {
  assert_eq!(path,"/proc/breenix/frames");Ok("site gpu/dma.rs:197 8013\nowner buffers 13598\n".into())
 }
}}
fn prepare(resources:Option<&libbreenix::gpu::abi::Caps>)->Vec<u8>{
BODY
 data
}
fn main(){let caps=libbreenix::gpu::abi::Caps{bytes:[3;512]};
 print!("{}",String::from_utf8(prepare(Some(&caps))).unwrap());}
'''.replace('BODY',body)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp); (p/'memory.rs').write_text(fixture)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'memory.rs'),'-o',str(p/'memory')],check=True)
            rows=subprocess.check_output([str(p/'memory')],text=True).splitlines()
        self.assertEqual(rows[0], 'startup and runtime probes')
        record=json.loads(rows[1].removeprefix('GT710_RESOURCES '))
        self.assertEqual(record['hex'], '03'*512)
        self.assertEqual(bytes.fromhex(record['frame_census_hex']), b'site gpu/dma.rs:197 8013\nowner buffers 13598\n')
if __name__=='__main__':unittest.main()
