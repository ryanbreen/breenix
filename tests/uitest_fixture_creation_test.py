"""Create the real private fixture under restrictive masks and count redundant chmods."""
from pathlib import Path
import os
import subprocess
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class Creation(unittest.TestCase):
    def test_exact_modes_mask_restoration_and_no_file_rechmod(self):
        path='libs/libuitest/src/fixtures.rs'
        old=os.environ.get('UITEST_OLD_COMMIT')
        source=subprocess.check_output(['git','show',f'{old}:{path}'],cwd=ROOT,text=True) if old else (ROOT/path).read_text()
        constants=source[source.index('pub const FONT_CONFIG:'):source.index('pub fn digest(')]
        run_rust(self,r'''
use std::{path::Path,os::unix::fs::PermissionsExt,sync::atomic::{AtomicUsize,Ordering}};
static CHMODS:AtomicUsize=AtomicUsize::new(0);
unsafe extern "C"{fn umask(mask:u32)->u32;}
mod fs {
 pub use std::fs::{symlink_metadata,remove_dir_all,remove_file,create_dir_all,write,Permissions};
 pub fn set_permissions(p:impl AsRef<std::path::Path>,v:Permissions)->std::io::Result<()>{super::CHMODS.fetch_add(1,super::Ordering::Relaxed);std::fs::set_permissions(p,v)}
}
@CONSTANTS@
@PREPARE@
fn inspect(root:&Path){for entry in std::fs::read_dir(root).unwrap(){let e=entry.unwrap();let m=e.metadata().unwrap();assert_eq!(m.permissions().mode()&0o7777,if m.is_dir(){0o755}else{0o644});if m.is_dir(){inspect(&e.path())}}}
#[test]fn private_tree_modes_and_failure_restore(){
 let base=std::env::temp_dir().join(format!("ui-create-{}",std::process::id()));std::fs::create_dir(&base).unwrap();
 // Keep the same syscall-spy boundary when the repaired producer needs dirs only.
 fs::set_permissions(&base,fs::Permissions::from_mode(0o755)).unwrap();
 let previous=unsafe{umask(0o077)};
 for mask in [0o077,0o000,0o027]{
  unsafe{umask(mask)};CHMODS.store(0,Ordering::Relaxed);
  let root=base.join("session");prepare(&root).unwrap();
  assert_eq!(unsafe{umask(mask)},mask,"fixture leaked its creation mask");
  assert_eq!(std::fs::metadata(&root).unwrap().permissions().mode()&0o7777,0o755);inspect(&root);
  assert_eq!(std::fs::read(root.join("etc/fonts.conf")).unwrap(),FONT_CONFIG);
  assert_eq!(std::fs::read(root.join("ui-clock")).unwrap().len(),64);
  assert_eq!(CHMODS.load(Ordering::Relaxed),6,"new files were re-chmodded after creation");
  std::fs::remove_dir_all(root).unwrap();
 }
 std::fs::write(base.join("file"),b"x").unwrap();
 assert!(prepare(&base.join("file/child")).is_err());assert_eq!(unsafe{umask(previous)},0o027);
 std::fs::remove_dir_all(base).unwrap();
}
'''.replace('@CONSTANTS@',constants).replace('@PREPARE@',item(source,'pub fn prepare(')))
if __name__=='__main__':unittest.main()
