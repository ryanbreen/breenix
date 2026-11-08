"""Exact desktop rows avoid the process libc's byte comparator without overreading."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class WordRows(unittest.TestCase):
 def test_word_comparison_retains_every_byte_and_tail(self):
  source=(ROOT/'userspace/programs/src/bwm/compositor.rs').read_text()
  body=item(source,'fn equal_row(') if 'fn equal_row(' in source else 'fn equal_row(a:&[u8],b:&[u8])->bool{a==b}'
  run_rust(self,r'''
use std::sync::atomic::{AtomicUsize,Ordering};
static BYTE_WORK:AtomicUsize=AtomicUsize::new(0);
#[no_mangle]pub unsafe extern "C" fn bcmp(a:*const u8,b:*const u8,n:usize)->i32{BYTE_WORK.fetch_add(n,Ordering::Relaxed);for i in 0..n{if a.add(i).read_volatile()!=b.add(i).read_volatile(){return a.add(i).read_volatile() as i32-b.add(i).read_volatile() as i32;}}0}
#[no_mangle]pub unsafe extern "C" fn memcmp(a:*const u8,b:*const u8,n:usize)->i32{bcmp(a,b,n)}
#[inline(never)]
@BODY@
#[test]fn exact_rows_and_cost(){
 for offset in 0..8{for n in 0..65{let a=vec![0xa5;n+offset];let mut b=a.clone();assert!(equal_row(&a[offset..],&b[offset..]));for i in offset..n+offset{b[i]^=1;assert!(!equal_row(&a[offset..],&b[offset..]),"missed byte {i} of {n}");b[i]^=1;}}}
 let a=vec![0x73;2560*4];let mut b=a.clone();BYTE_WORK.store(0,Ordering::Relaxed);
 assert!(equal_row(std::hint::black_box(&a),std::hint::black_box(&b)));let work=BYTE_WORK.load(Ordering::Relaxed);assert_eq!(work,0,"equal desktop row compared {work} bytes through scalar libc bcmp");
 b[10239]^=0x80;assert!(!equal_row(&a,&b));assert!(!equal_row(&a,&b[..10239]));
}
'''.replace('@BODY@',body))
if __name__=='__main__':unittest.main()
