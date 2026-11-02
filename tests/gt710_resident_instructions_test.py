"""Keep resident instruction lines while retaining data/constant invalidation."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ResidentInstructions(unittest.TestCase):
    def test_retained_catalogue_does_not_invalidate_instruction_lines(self):
        encoder = (ROOT / 'kernel/src/gpu/gk208/encode.rs').read_text().replace(
            'include_bytes!("caps/integer.inputs")',
            'include_bytes!("' + str(ROOT / 'kernel/src/gpu/gk208/caps/integer.inputs') + '")')
        fixture = r'''
pub fn check_resident_instructions() {
 let mut words=vec![
  0x20010000,0xa197, // object binding
  0x20010369,0x1011, // NO_WFI instruction/data/constant invalidation
  0x20020850,0x11,0x30, // vertex shader binding
  0x20010369,1, // instruction-only invalidation
  0x20020618,0,6, // draw
  0x20010044,0, // explicit idle before parking
  0x20010369,0x1011, // parking's inherited invalidation
  0x20010369,1,
  0x200106c0,7, // receipt action
  0x20010087,0x1011, // upload/bootstrap WFI form remains intact
 ];
 retain_resident_instruction_cache(&mut words).unwrap();
 assert_eq!(words,vec![0x20010000,0xa197,0x20010369,0x1010,
  0x20020850,0x11,0x30,0x20020618,0,6,0x20010044,0,
  0x20010369,0x1010,0x200106c0,7,0x20010087,0x1011]);
 let once=words.clone();retain_resident_instruction_cache(&mut words).unwrap();assert_eq!(words,once);
 assert!(retain_resident_instruction_cache(&mut vec![0x20020369,1]).is_err());
}
'''
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-resident-instructions-') as directory:
            p = Path(directory)
            (p / 'encode.rs').write_text(encoder + fixture)
            (p / 'check.rs').write_text('''extern crate alloc;
#[allow(dead_code)] // Import the production catalogue for this packet check.
#[path="SHADERS"]pub mod shaders;
#[allow(dead_code)]
#[path="CAPS"]pub mod caps;
#[allow(dead_code)]
mod encode;
fn main(){encode::check_resident_instructions();}
'''.replace('SHADERS', str(ROOT/'kernel/src/gpu/gk208/gr/shaders.rs'))
                .replace('CAPS', str(ROOT/'kernel/src/gpu/gk208/caps.rs')))
            subprocess.run(priority+['rustc','--edition=2021','--crate-name','gpu_abi','--crate-type','rlib','--cfg','feature="alloc"','-Dwarnings',str(ROOT/'libs/gpu-abi/src/lib.rs'),'-o',str(p/'libgpu_abi.rlib')],check=True)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings','--extern',f'gpu_abi={p/"libgpu_abi.rlib"}',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)


if __name__ == '__main__':
    unittest.main()
