"""A saved readback cannot bypass the production non-case binding filter."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from screen_evidence_test import function
ROOT = Path(__file__).resolve().parents[1]

class Binding(unittest.TestCase):
    def test_console_and_runner_do_not_publish_reused_pictures(self):
        source = (ROOT / 'libs/libbreenix/src/suite_screen.rs').read_text()
        fixture = r'''
#![allow(dead_code)]
use std::sync::atomic::{AtomicU64,Ordering};
use std::string::String;
static RUNNER:AtomicU64=AtomicU64::new(99);
static RUNNER_RESOURCE:AtomicU64=AtomicU64::new(20);
static BOUND:AtomicU64=AtomicU64::new(10);
mod gpu {
 pub mod abi {pub mod backend {pub const GK208:u32=6;} pub mod screen { ABI }
 pub mod record {pub const SIZE:usize=1024;pub const CONTEXT:usize=0;pub const RESOURCE:usize=8;pub const TARGET_WIDTH:usize=16;pub const TARGET_HEIGHT:usize=20;}
 #[derive(Clone,Copy)] pub struct Mode{pub width:u32,pub height:u32,pub scale:u32,pub reserved:u32}}
 pub struct Modes{pub binding_known:u32,pub scanout_resource:u64,pub console_resource:u64}
 pub struct Caps{pub backend:u32}
 pub fn modes()->(i64,Modes){(0,Modes{binding_known:1,scanout_resource:crate::BOUND.load(crate::Ordering::Relaxed),console_resource:10})}
 pub fn caps()->(i64,Caps){(0,Caps{backend:6})}
 pub fn current_mode()->Result<abi::Mode,()>{Ok(crate::mode())}
}
use gpu::abi::{Mode,record as r};
type Snapshot=Result<([u8;32],Mode,Mode),String>;
fn mode()->Mode{Mode{width:2,height:1,scale:1,reserved:0}}
fn reuse_snapshot()->Option<Snapshot>{Some(Ok(([7;32],mode(),mode())))}
fn quad(_: &[u8],_:usize)->u64{30}
fn word(_: &[u8],_:usize)->u32{2}
fn validate(_: &[u8;r::SIZE],_:Mode,_:u32,_:u64)->Result<(),&'static str>{Ok(())}
fn canonical_hash(_: &mut [u8],_:Mode)->Result<[u8;32],&'static str>{Ok([7;32])}
fn endpoint_error(_:i32,_:&[u8])->String{String::from("readback-failed")}
POLICY
SNAPSHOT
#[test]fn excludes(){for resource in [0,10,20]{
 BOUND.store(resource,Ordering::Relaxed);
 assert!(matches!(snapshot(),Err(e) if e=="no-case-scanout"),"published resource {resource}");
}}
'''.replace('ABI',(ROOT/'libs/gpu-abi/src/screen.rs').read_text()).replace('POLICY',function(source,'pub(super) fn non_case_binding(')).replace('SNAPSHOT',function(source,'fn snapshot(')).replace('pub(super)', 'pub')
        with tempfile.TemporaryDirectory() as directory:
            p=Path(directory);(p/'test.rs').write_text(fixture)
            priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
            subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings',str(p/'test.rs'),'-o',str(p/'test')],check=True)
            subprocess.run([str(p/'test')],check=True)
if __name__=='__main__':unittest.main()
