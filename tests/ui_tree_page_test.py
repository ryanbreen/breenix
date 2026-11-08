#!/usr/bin/env python3
"""Run BreenGL's tree-page writer against disjoint acquired/front pages."""
from pathlib import Path
import subprocess
import sys
import tempfile
ROOT = Path(__file__).resolve().parents[1]
source = Path(sys.argv[1]).read_text() if len(sys.argv) > 1 else (ROOT / "libs/breengel/src/window.rs").read_text()
start = source.find("    pub fn write_tree(")
assert start >= 0, "BreenGL has no acquired tree-page writer"
brace = source.index("{", start)
depth = 1
end = brace + 1
while depth:
    depth += (source[end] == "{") - (source[end] == "}")
    end += 1
method = source[start:end]
harness = r'''
#[derive(Debug,PartialEq,Eq)] enum Errno { EINVAL, EIO }
#[derive(Debug,PartialEq,Eq)] enum Error { Os(Errno) }
struct Acquired {buffer:u32}
struct Window {tree:u64,tree_bytes:usize,next:u32,calls:usize,refuse:bool}
impl Window {
 fn acquire(&mut self, deadline:u64)->Result<Acquired,Error> {
  assert_eq!(deadline,u64::MAX);self.calls+=1;
  if self.refuse {return Err(Error::Os(Errno::EIO));}
  Ok(Acquired {buffer:self.next})
 }
 METHOD
}
#[test] fn acquired_page_only_and_refusals_preserve_front() {
 let mut pages=[0xa5u8;3*64];
 let mut window=Window {tree:pages.as_mut_ptr() as u64,tree_bytes:64,next:2,calls:0,refuse:false};
 let serialized=b"root generation 7\0";
 window.write_tree(serialized).unwrap();
 assert_eq!(window.calls,1);
 assert_eq!(&pages[..128],&[0xa5u8;128]);
 assert_eq!(&pages[128..128+serialized.len()],serialized);
 assert_eq!(&pages[128+serialized.len()..],vec![0xa5;64-serialized.len()].as_slice());
 let before=pages;
 assert_eq!(window.write_tree(&[0;65]),Err(Error::Os(Errno::EINVAL)));
 assert_eq!(window.calls,1);assert_eq!(pages,before);
 window.refuse=true;
 assert_eq!(window.write_tree(serialized),Err(Error::Os(Errno::EIO)));
 assert_eq!(pages,before);
}
'''.replace("METHOD", method)
with tempfile.TemporaryDirectory(prefix="ui-tree-page-") as directory:
    path = Path(directory) / "check.rs"
    path.write_text(harness)
    binary = Path(directory) / "check"
    command = ["rustc", "--edition=2021", "--test", "-Dwarnings", str(path), "-o", str(binary)]
    if sys.platform == "darwin": command = ["taskpolicy", "-b", *command]
    subprocess.run(command, check=True, env=__import__('os').environ.copy())
    subprocess.run([str(binary)], check=True)
