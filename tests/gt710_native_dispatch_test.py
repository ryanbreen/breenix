"""Execute device context builders at the native dispatch boundary."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))
from gt710_gr_live_unwind_test import item

class NativeDispatch(unittest.TestCase):
    def test_context_builders_and_native_identities(self):
        source = (ROOT / "kernel/src/gpu/dev.rs").read_text()
        functions = "\n".join(item(source, "pub fn " + name + "(") for name in
                              ["ctx_create", "ctx_destroy", "ctx_attach", "context_id", "resource_id", "set_scanout", "flush_resource"])
        functions = functions.replace('#[cfg(target_arch = "x86_64")]', "")
        fixture = r'''
#![allow(dead_code)] // Stub backends exercise the extracted production dispatch.
mod backend {pub const VIRGL:u32=4;pub const SVGA3D_DX:u32=5;pub const GK208:u32=6;}
fn backend()->u32{backend::GK208}
#[derive(Clone,Copy)] pub struct Tag;
#[derive(Debug,PartialEq)] pub enum Request {Virgl(u32),Svga(u32),Gk208(u32)}
mod svga {
 use super::Tag;
 pub fn ctx_create(_:Tag,_:u32)->Option<u32>{panic!("SVGA context")}
 pub fn ctx_destroy(_:Tag,_:u32)->u32{panic!("SVGA context")}
 pub fn ctx_attach(_:Tag)->u32{panic!("SVGA attachment")}
 pub fn set_scanout(_:Tag,_:u32,_:u32,_:u32)->Option<u32>{panic!("SVGA scanout")}
 pub fn flush_resource(_:Tag,_:u32,_:u64,_:u32,_:u32)->Option<u32>{panic!("SVGA flush")}
 pub fn cid_of(_:u32)->Option<u32>{None} pub fn sid_of(_:u32)->Option<u32>{None}
}
mod virgl {
 use super::Tag;
 pub fn set_scanout(_:Tag,_:u32,_:u32,_:u32,_:u32)->u32{panic!("VirGL scanout")}
 pub fn flush_resource(_:Tag,_:u32,_:u64,_:u32,_:u32,_:u32)->u32{panic!("VirGL flush")}
 pub fn ctx_create(_:Tag,_:u32)->u32{panic!("VirGL context")}
 pub fn ctx_destroy(_:Tag,_:u32)->u32{panic!("VirGL context")}
 pub fn ctx_attach(_:Tag,_:u32,_:u32)->u32{panic!("VirGL attachment")}
}
mod gk208 {
 pub fn set_scanout(_:crate::Tag,c:u32,r:u32,w:u32,h:u32)->Option<u32>{assert_eq!((c,r,w,h),(7,19,1280,1024));Some(300)}
 pub fn flush_resource(_:crate::Tag,c:u32,f:u64,r:u32,w:u32,h:u32)->Option<u32>{assert_eq!((c,f,r,w,h),(7,91,19,1280,1024));Some(400)}
 pub mod resource {
 use crate::Tag;
 pub fn ctx_create(_:Tag,c:u32)->Option<u32>{Some(c+100)}
 pub fn ctx_destroy(_:Tag,c:u32)->Option<u32>{Some(c+200)}
 pub fn ctx_attach(_:Tag,c:u32,r:u32)->Option<u32>{Some(c+r)}
 pub fn context_id(c:u32)->Option<u32>{Some(c+1000)}
 pub fn resource_id(r:u32)->Option<u32>{Some(r+2000)}
}}
// FUNCTIONS
fn main(){
 assert_eq!(set_scanout(Tag,7,19,1280,1024),Some(Request::Gk208(300)));
 assert_eq!(flush_resource(Tag,7,91,19,1280,1024),Some(Request::Gk208(400)));
 assert_eq!(ctx_create(Tag,7),Some(Request::Gk208(107)));
 assert_eq!(ctx_attach(Tag,7,19),Some(Request::Gk208(26)));
 assert_eq!(ctx_destroy(Tag,7),Some(Request::Gk208(207)));
 assert_eq!(context_id(7),Some(1007));assert_eq!(resource_id(19),Some(2019));
}
'''.replace("// FUNCTIONS", functions)
        priority = ["taskpolicy", "-b"] if sys.platform == "darwin" else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / "dispatch.rs").write_text(fixture)
            subprocess.run(priority + ["rustc", "--edition=2021", "-Dwarnings", str(path / "dispatch.rs"), "-o", str(path / "dispatch")], check=True)
            subprocess.run([str(path / "dispatch")], check=True)

if __name__ == "__main__": unittest.main()
