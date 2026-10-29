"""Check native catalogue refusals carry the ABI command index and BAD_PIPELINE."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item

class PipelineRefusal(unittest.TestCase):
    def test_unsupported_draw_is_an_indexed_abi_refusal(self):
        source = (ROOT / 'kernel/src/gpu/gk208/encode.rs').read_text()
        method = item(source, 'pub fn pipeline_refusal(')
        harness = r'''
use gpu_abi::{draw, pipeline, stream::{self, cmd, Words}};
// METHOD
fn main() {
 let mut words=Vec::new(); stream::encode::begin(&mut words,1);
 stream::encode::clear(&mut words,0);
 stream::encode::draw(&mut words,pipeline::SOLID,0,&[[0,0,4,4]]);
 assert_eq!(pipeline_refusal(words.as_slice()),None);
 let mut params=[0;16];params[..3].copy_from_slice(&[0,0xffffffff,4]);
 draw::encode(&mut words,pipeline::GRADIENT_V,0,0,0,0,&[draw::Instance {rect:[0,0,4,4],params}]);
 assert_eq!(pipeline_refusal(words.as_slice()),Some(stream::Refusal {index:3,error:stream::error::BAD_PIPELINE}));
}
'''.replace('// METHOD', method)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp);(path/'refusal.rs').write_text(harness)
            subprocess.run(priority+['rustc','--edition=2021','--crate-name','gpu_abi','--crate-type','rlib',
                str(ROOT/'libs/gpu-abi/src/lib.rs'),'-o',str(path/'libgpu_abi.rlib')],check=True)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings','--extern',f'gpu_abi={path/"libgpu_abi.rlib"}',
                str(path/'refusal.rs'),'-o',str(path/'refusal')],check=True)
            subprocess.run([str(path/'refusal')],check=True)
if __name__=='__main__': unittest.main()
