"""Exercise the guest profile cases with admitted but unmeasured native CAPS."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class ProfileSources(unittest.TestCase):
    def test_guest_cases_require_measured_native_formats_and_integer_ops(self):
        source = (ROOT / 'userspace/programs/src/suite_gpu/device.rs').read_text()
        functions = '\n'.join(item(source, marker) for marker in [
            'fn raw(', 'pub fn profile_formats(', 'pub fn profile_integer_ops(',
            'pub fn profile_record('])
        # Include the production provenance predicate when present.
        if 'fn measured_sources(' in source:
            functions += '\n' + item(source, 'fn measured_sources(')
        fixture = r'''
use gpu_abi as abi;
use abi::{capability,format,pipeline};
use std::cell::RefCell;
type CaseResult=Result<(),String>;
thread_local! { static CAPS:RefCell<abi::Caps>=RefCell::new(admitted(6)); }
fn caps()->Result<abi::Caps,String>{Ok(CAPS.with(|c|*c.borrow()))}
fn begin()->Result<(),String>{Ok(())}
fn pass()->CaseResult{Ok(())}
fn ensure(ok:bool,why:impl FnOnce()->String)->CaseResult{if ok{Ok(())}else{Err(why())}}
fn value(_: &str,_:i64,_:&str,_:Option<(i64,i64)>){ }
fn u32_at(b:&[u8],i:usize)->u32{u32::from_le_bytes(b[i..i+4].try_into().unwrap())}
fn errno(r:i64)->i64{r}
const EINVAL:i64=22;
mod gpu {pub unsafe fn call(_:u32,_:*mut u8,_:usize)->i64{-22}}
fn admitted(backend:u32)->abi::Caps{
 let mut c:abi::Caps=unsafe{std::mem::zeroed()};
 c.version=1;c.size=512;c.backend=backend;c.catalogue_version=1;
 c.pipelines=1<<pipeline::SOLID;
 c.formats=[abi::FormatCaps{format:format::B8G8R8A8,flags:3},
            abi::FormatCaps{format:format::R8G8B8A8,flags:3},
            abi::FormatCaps{format:format::R8,flags:1}];
 c.profile=abi::profile::INTEGER_OPS|abi::profile::TEXEL_FETCH|abi::profile::REGION_COPY;
 c.shading_level=if backend==6{0}else{1};
 let mut packed=0u64;
 for id in capability::BGRA_RENDER..=capability::REGION_COPY{packed|=4<<((id-1)*3);}
 c.capability_origins=[packed as u32,(packed>>32) as u32];c
}
fn set_origin(c:&mut abi::Caps,id:u32,origin:u32){
 let shift=(id-1)*3;
 let packed=c.capability_origins[0] as u64|((c.capability_origins[1] as u64)<<32);
 let packed=(packed&!(7<<shift))|((origin as u64)<<shift);
 c.capability_origins=[packed as u32,(packed>>32) as u32];
}
FUNCTIONS
#[test]fn native_sources_must_be_executable_probes(){
 for (case,ids) in [
  (profile_formats as fn()->CaseResult,capability::BGRA_RENDER..=capability::R8_SAMPLE),
  (profile_integer_ops,capability::INTEGER_OPS..=capability::REGION_COPY),
  (profile_record,capability::BGRA_RENDER..=capability::REGION_COPY)]{
  CAPS.with(|v|*v.borrow_mut()=admitted(6));assert!(case().is_ok());
  for id in ids {for origin in [0,1,2,3,5,6,7]{
   let mut c=admitted(6);set_origin(&mut c,id,origin);CAPS.with(|v|*v.borrow_mut()=c);
   assert!(case().is_err(),"capability {id} accepted non-probe origin {origin}");
  }}
 }
}
#[test]fn virtual_backends_keep_their_capset_contract(){
 for backend in [4,5]{let mut c=admitted(backend);c.capability_origins=[0;2];
  CAPS.with(|v|*v.borrow_mut()=c);
  assert!(profile_formats().is_ok());assert!(profile_integer_ops().is_ok());assert!(profile_record().is_ok());
 }
}
'''.replace('FUNCTIONS', functions)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                '--crate-name', 'gpu_abi', '--crate-type', 'rlib',
                str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(path / 'libgpu_abi.rlib')], check=True)
            (path / 'profile.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--test',
                '--extern', f'gpu_abi={path / "libgpu_abi.rlib"}', str(path / 'profile.rs'),
                '-o', str(path / 'profile')], check=True)
            subprocess.run([str(path / 'profile')], check=True)


if __name__ == '__main__':
    unittest.main()
