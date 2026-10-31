"""Exercise the CPU staging owner with recycled pages and injected map failures."""
from pathlib import Path
import subprocess,sys,tempfile,unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from console_gpu_admission_test import x86
class Reuse(unittest.TestCase):
    def test_recycled_pages_and_failed_mapping_teardown(self):
        dma=(ROOT/'kernel/src/gpu/dma.rs').read_text()
        start=dma.index('/// CPU-only contiguous staging storage.') if '/// CPU-only contiguous staging storage.' in dma else dma.index('/// CPU-only virtually contiguous staging storage.')
        code=x86(dma[start:dma.index('/// Whole frames, in order, owned until dropped.',start)]).replace('pub(super)', 'pub(crate)')
        fixture=r'''
#![allow(dead_code)] // Complete mock APIs cover the old and repaired allocation paths.
use std::{collections::{BTreeMap,BTreeSet},ptr::NonNull,sync::{Mutex,OnceLock,atomic::{AtomicU64,Ordering}}};
mod spin { pub struct Mutex<T>(std::sync::Mutex<T>); impl<T> Mutex<T> { pub const fn new(v:T)->Self {Self(std::sync::Mutex::new(v))} pub fn lock(&self)->std::sync::MutexGuard<'_,T> {self.0.lock().unwrap()} } }
const PAGE:usize=4096;const LIMIT:usize=1024;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]struct PhysFrame(usize);
struct PhysAddr(u64);impl PhysAddr{fn as_u64(&self)->u64{self.0}}
impl PhysFrame{fn start_address(self)->PhysAddr{PhysAddr((self.0 as u64+1)*PAGE as u64)}}
#[derive(Clone,Copy)]struct VirtAddr(u64);
impl VirtAddr{
 fn new(v:u64)->Self{Self(v)}fn as_u64(self)->u64{self.0}
 fn as_mut_ptr<T>(self)->*mut T{let slot=((self.0-memory::layout::KERNEL_BUFFER_REGION_BASE)/(128*1024*1024))as usize;unsafe{ram().add(slot*(2<<20))as *mut T}}
}
impl std::ops::Add<u64> for VirtAddr{type Output=Self;fn add(self,v:u64)->Self{Self(self.0+v)}}
struct PageTableFlags;impl PageTableFlags{const PRESENT:Self=Self;const WRITABLE:Self=Self;const NO_EXECUTE:Self=Self;}
impl std::ops::BitOr for PageTableFlags{type Output=Self;fn bitor(self,_:Self)->Self{Self}}
#[derive(Default)]struct State{next:usize,free:Vec<PhysFrame>,live:BTreeSet<usize>,maps:BTreeMap<u64,PhysFrame>,events:Vec<&'static str>,fail_map:Option<usize>,map_count:usize}
static STATE:Mutex<Option<State>>=Mutex::new(None);
fn state<R>(f:impl FnOnce(&mut State)->R)->R{f(STATE.lock().unwrap().as_mut().unwrap())}
static MOCK_EPOCH:AtomicU64=AtomicU64::new(0);
fn reset(){CACHE_RESET MOCK_EPOCH.store(0,Ordering::Relaxed);*STATE.lock().unwrap()=Some(State::default());}
fn ram()->*mut u8{static RAM:OnceLock<usize>=OnceLock::new();*RAM.get_or_init(||Box::into_raw(vec![0u8;(2<<20)*64].into_boxed_slice())as *mut u8 as usize)as *mut u8}
fn frame_va(f:&PhysFrame)->u64{unsafe{ram().add(f.0*PAGE)as u64}}
mod frame_allocator{
 pub fn allocate_frame()->Option<super::PhysFrame>{super::state(|s|{let f=if let Some(f)=s.free.pop(){f}else{if s.next==super::LIMIT{return None;}let f=super::PhysFrame(s.next);s.next+=1;f};assert!(s.live.insert(f.0));Some(f)})}
 pub fn allocate_contiguous_frames(n:usize,out:&mut[Option<super::PhysFrame>])->Option<super::PhysFrame>{super::state(|s|{if s.next+n>super::LIMIT{return None;}let first=super::PhysFrame(s.next);for slot in out.iter_mut().take(n){let f=super::PhysFrame(s.next);s.next+=1;assert!(s.live.insert(f.0));*slot=Some(f);}Some(first)})}
 pub fn deallocate_frame(f:super::PhysFrame){super::state(|s|{assert!(!s.maps.values().any(|v|*v==f),"a mapped frame was freed");assert!(s.live.remove(&f.0));s.events.push("free");s.free.push(f);});}
}
mod memory{
 pub mod layout{pub const KERNEL_BUFFER_REGION_BASE:u64=0xffffc90100000000;}
 pub mod kernel_page_table{
  pub unsafe fn map_kernel_page(v:super::super::VirtAddr,p:super::super::PhysAddr,_:super::super::PageTableFlags)->Result<(),&'static str>{super::super::state(|s|{let n=s.map_count;s.map_count+=1;if s.fail_map==Some(n){return Err("injected map failure");}let f=super::super::PhysFrame((p.0 as usize/4096)-1);assert!(s.maps.insert(v.0,f).is_none());s.events.push("map");Ok(())})}
  pub unsafe fn unmap_kernel_page(v:super::super::VirtAddr)->Result<Option<super::super::PhysFrame>,&'static str>{super::super::state(|s|{s.events.push("unmap");Ok(s.maps.remove(&v.0))})}
 }
 pub mod tlb{pub fn flush_all(){super::super::state(|s|s.events.push("flush"));}}
}
struct FrameRun{frames:Vec<Option<PhysFrame>>,owner:Option<()>}
impl Drop for FrameRun{fn drop(&mut self){let _=self.owner.take();for f in self.frames.iter_mut().filter_map(Option::take){frame_allocator::deallocate_frame(f);}}}
struct DmaBox{bytes:NonNull<u8>,_frames:FrameRun,address:u64,len:usize}
// Match the production owner's Send contract; every mock mapping is unique.
unsafe impl Send for DmaBox {}
impl DmaBox{
 fn len(&self)->usize{assert_eq!(self.address,0);self.len}
 fn as_ref(&self)->&[u8]{unsafe{std::slice::from_raw_parts(self.bytes.as_ptr(),self.len)}}
 fn as_mut(&mut self)->&mut[u8]{unsafe{std::slice::from_raw_parts_mut(self.bytes.as_ptr(),self.len)}}
}
CODE
#[test]fn repeated_staging_reuses_returned_physical_pages(){
 reset();for n in 0..128{let mut b=CpuBox::try_new(PAGE*4).unwrap_or_else(||panic!("allocation {n} exhausted the frontier despite returned pages"));assert_eq!(b.len(),PAGE*4);assert!(b.as_ref().iter().all(|&v|v==0));b.as_mut()[0]=1;drop(b);}
 state(|s|{assert!(s.live.is_empty());assert!(s.maps.is_empty());assert!(s.next<=8,"physical frontier grew to {}",s.next);});
}
#[test]fn repeated_frame_preparation_reuses_one_bounded_mapping(){
 reset();let request=(1<<20)+64*1024;
 for _ in 0..128 {
  let mut b=CpuBox::try_new(request).unwrap();
  assert!(b.len()>=request);assert!(b.as_ref()[..request].iter().all(|&v|v==0));
  b.as_mut()[0]=9;drop(b);
 }
 let mapped=state(|s|s.map_count);assert_eq!(mapped,512,"repeated frame preparation recreated page tables instead of reusing a bounded staging mapping");
}
#[test]fn cache_retains_only_one_mapping_and_reuses_clean_bytes(){
 reset();let request=(1<<20)+64*1024;
 let mut a=CpuBox::try_new(request).unwrap();let b=CpuBox::try_new(request).unwrap();a.as_mut()[7]=17;
 drop(a);drop(b);
 let held=state(|s|(s.maps.len(),s.live.len(),s.map_count));assert_eq!(held,(512,512,1024));
 let c=CpuBox::try_new(request).unwrap();assert_eq!(c.as_ref()[7],0);drop(c);
 let held=state(|s|(s.maps.len(),s.live.len(),s.map_count));assert_eq!(held,(512,512,1024));
}
#[test]fn failed_mapping_unmaps_and_flushes_before_releasing_frames(){
 reset();state(|s|s.fail_map=Some(2));assert!(CpuBox::try_new(PAGE*4).is_none());
 state(|s|{assert!(s.live.is_empty());assert!(s.maps.is_empty());let f=s.events.iter().position(|v|*v=="flush").unwrap();let r=s.events.iter().position(|v|*v=="free").unwrap();assert!(f<r);});
 state(|s|s.fail_map=None);let b=CpuBox::try_new(PAGE*4).expect("failed map leaked a slot");drop(b);
}
'''.replace('CODE',code).replace('CACHE_RESET', 'drop(CPU_BUFFER_CACHE.lock().take());' if 'static CPU_BUFFER_CACHE:' in code else '')
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'reuse.rs').write_text(fixture)
            subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings',str(p/'reuse.rs'),'-o',str(p/'reuse')],check=True)
            subprocess.run([str(p/'reuse'),'--nocapture','--test-threads=1'],check=True)
if __name__=='__main__':unittest.main()
