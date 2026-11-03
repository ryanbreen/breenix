"""Exercise the stage's production observation-boundary match on the host."""
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / 'libs/libbreenix/src/suite_stage/checks.rs').read_text()
start = source.index('match (rec.backend, rec.flags, rec.boundary) {')
end = source.index('\n                }', start) + len('\n                }')
production_match = source[start:end]
fixture = r'''
struct Record { backend: u32, flags: u32, boundary: u32 }
fn accepts(rec: Record) -> Result<(), String> {
    PRODUCTION_MATCH
    Ok(())
}
#[test]
fn observation_boundary_contract() {
    for backend in [4, 5] {
        for flags in [1, 3] {
            assert!(accepts(Record { backend, flags, boundary: 4 }).is_ok());
            assert!(accepts(Record { backend, flags, boundary: 5 }).is_err());
        }
        assert!(accepts(Record { backend, flags: 2, boundary: 4 }).is_err());
    }
    for flags in [1, 3] {
        assert!(accepts(Record { backend: 6, flags, boundary: 5 }).is_ok());
    }
    for (backend, flags, boundary) in [(6, 1, 4), (4, 1, 5), (6, 2, 5), (7, 1, 5)] {
        assert!(accepts(Record { backend, flags, boundary }).is_err());
    }
}
'''.replace('PRODUCTION_MATCH', production_match)
# Feed complete little-endian v2 wire records through the production parser,
# then through boundary-notice's production verdict, rather than constructing Seen.
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item
stage = (ROOT / 'libs/libbreenix/src/suite_stage.rs').read_text()
record = item((ROOT / 'libs/gpu-abi/src/lib.rs').read_text(), 'pub mod record {')
wire_fixture = r'''
use std::cell::RefCell;
thread_local! {static RECORD:RefCell<Vec<u8>>=const {RefCell::new(Vec::new())};}
RECORD_MODULE
SEEN
WORD
QUAD
MODE
OBSERVE
mod gpu {
 pub mod abi { pub struct Mode {pub width:u32,pub height:u32} }
 pub fn current_mode()->Result<abi::Mode,i64>{Ok(abi::Mode{width:2,height:1})}
}
mod abi {
 pub mod op {pub const SCANOUT_READBACK:u32=24;}
 pub struct Readback {pub dst:u64,pub dst_len:u64,pub record:u64,pub record_len:u32,pub reason:u32}
 impl Readback {pub const ZERO:Self=Self{dst:0,dst_len:0,record:0,record_len:0,reason:0};}
}
use abi::op;
mod win {
 pub fn call(op:u32,rb:&mut super::abi::Readback)->Result<(),i32>{
  assert_eq!(op,super::op::SCANOUT_READBACK);
  assert_eq!(rb.dst_len,8);
  super::RECORD.with(|r| unsafe {std::ptr::copy_nonoverlapping(r.borrow().as_ptr(),rb.record as *mut u8,rb.record_len as usize)});
  Ok(())
 }
}
fn score_record(bytes:Vec<u8>)->Result<(),String>{
 RECORD.with(|r|*r.borrow_mut()=bytes);
 let rec=observe(&mut Vec::new())?;
 assert_eq!((rec.width,rec.height,rec.present_fence),(2,1,9));
 PRODUCTION_MATCH
 Ok(())
}
#[test] fn boundary_notice_rejects_native_host_boundary_wire_record(){
 let mut rec=vec![0;record::SIZE];
 for (at,value) in [(record::VERSION_OFF,2u32),(record::SIZE_OFF,1024),
  (record::BACKEND,6),(record::BOUNDARY,5),(record::FLAGS,1),
  (record::WIDTH,2),(record::HEIGHT,1),(record::PRESENT_FENCE,9)] {
  rec[at..at+4].copy_from_slice(&value.to_le_bytes());
 }
 assert!(score_record(rec.clone()).is_ok());
 rec[record::BOUNDARY..record::BOUNDARY+4].copy_from_slice(&4u32.to_le_bytes());
 assert_eq!(score_record(rec.clone()),Err("invalid observation boundary combination".into()));
 rec[record::BACKEND..record::BACKEND+4].copy_from_slice(&4u32.to_le_bytes());
 assert!(score_record(rec).is_ok());
}
'''
for key,value in [('RECORD_MODULE',record),('SEEN',item(stage,'pub(crate) struct Seen {')),
                  ('WORD',item(stage,'fn word(')),('QUAD',item(stage,'fn quad(')),
                  ('MODE',item(stage,'fn mode(')),('OBSERVE',item(stage,'pub(crate) fn observe(')),
                  ('PRODUCTION_MATCH',production_match)]:
    wire_fixture=wire_fixture.replace(key,value)
fixture += wire_fixture
with tempfile.TemporaryDirectory() as directory:
    path = Path(directory)
    (path / 'test.rs').write_text(fixture)
    priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
    subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                               str(path / 'test.rs'), '-o', str(path / 'test')], check=True)
    subprocess.run([str(path / 'test')], check=True, timeout=10)
