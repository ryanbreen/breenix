"""Verified file identities survive session forks and invalidate on file changes."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class PinCache(unittest.TestCase):
 def test_identity_reuse_mutation_replacement_and_fork(self):
  s=(ROOT/'libs/libuitest/src/fixtures.rs').read_text().replace('use libpixcheck::{sha256, Sha256};','use crate::libpixcheck::{sha256, Sha256};')
  if 'fn pin_now_seconds(' in s:s=s.replace(item(s,'fn pin_now_seconds('),'fn pin_now_seconds()->i64{crate::NOW.load(crate::Ordering::SeqCst)}')
  coarse='''let mut identity=FileIdentity::of(&std::fs::metadata(&path).unwrap());identity.mtime=crate::NOW.load(crate::Ordering::SeqCst);identity.ctime=identity.mtime;identity.mtime_nsec=0;identity.ctime_nsec=0;assert!(!identity.stable_before(identity.mtime));assert!(identity.stable_before(identity.mtime+1));''' if 'struct FileIdentity' in s else ''
  code=r'''
use std::sync::atomic::{AtomicUsize,AtomicI64,Ordering};
static HASHES:AtomicUsize=AtomicUsize::new(0);static NOW:AtomicI64=AtomicI64::new(0);
#[path="@SHA@"]pub mod actual_sha;
mod libpixcheck{pub use crate::actual_sha::Sha256;pub fn sha256(v:&[u8])->[u8;32]{crate::HASHES.fetch_add(1,crate::Ordering::SeqCst);crate::actual_sha::sha256(v)}}
pub mod fixtures{@SOURCE@
#[cfg(test)]mod pin_cache_checks{use super::*;use std::os::unix::fs::MetadataExt;
#[test]fn session_identity_cache(){
 let root=std::env::temp_dir().join(format!("pin-cache-{}",std::process::id()));std::fs::create_dir_all(&root).unwrap();let path=root.join("input");std::fs::write(&path,b"pinned").unwrap();
 let pin=digest(b"pinned");crate::NOW.store(std::fs::metadata(&path).unwrap().ctime()+2,crate::Ordering::SeqCst);crate::HASHES.store(0,crate::Ordering::SeqCst);
 checked(&path,&pin).unwrap();checked(&path,&pin).unwrap();assert_eq!(crate::HASHES.load(crate::Ordering::SeqCst),1,"unchanged bytes were re-hashed in the next session");
 unsafe extern "C"{fn fork()->i32;fn waitpid(pid:i32,status:*mut i32,options:i32)->i32;fn _exit(code:i32)->!;fn alarm(seconds:u32)->u32;}
 let child=unsafe{fork()};assert!(child>=0);if child==0{unsafe{alarm(3)};let before=crate::HASHES.load(crate::Ordering::SeqCst);let good=checked(&path,&pin).is_ok()&&crate::HASHES.load(crate::Ordering::SeqCst)==before;unsafe{_exit(if good{0}else{1})}}
 let mut status=-1;assert_eq!(unsafe{waitpid(child,&mut status,0)},child);assert_eq!(status,0,"forked supervisor lost verified input identity");
 let before=crate::HASHES.load(crate::Ordering::SeqCst);std::fs::write(&path,b"mutate").unwrap();assert!(checked(&path,&pin).is_err(),"changed same-size input was accepted");assert!(crate::HASHES.load(crate::Ordering::SeqCst)>before);
 std::fs::write(&path,b"pinned").unwrap();checked(&path,&pin).unwrap();let before=crate::HASHES.load(crate::Ordering::SeqCst);let replacement=root.join("new");std::fs::write(&replacement,b"pinned").unwrap();std::fs::rename(&replacement,&path).unwrap();checked(&path,&pin).unwrap();assert!(crate::HASHES.load(crate::Ordering::SeqCst)>before,"replacement inode reused an old verification");assert!(checked(&path,&"0".repeat(64)).is_err());
 @COARSE@
 std::fs::remove_dir_all(root).unwrap();
}
}}
'''.replace('@SHA@',str(ROOT/'libs/libpixcheck/src/sha256.rs')).replace('@SOURCE@',s).replace('@COARSE@',coarse)
  run_rust(self,code)
if __name__=='__main__':unittest.main()
