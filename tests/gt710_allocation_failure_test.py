"""Large native buffers must refuse allocation failure without aborting the kernel."""
from pathlib import Path
import os,subprocess,sys,tempfile,unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item
from console_gpu_admission_test import x86
ALLOC=r'''
use std::alloc::{GlobalAlloc,Layout,System};
use std::sync::atomic::{AtomicBool,AtomicUsize,Ordering};
struct Alloc;
static ARMED:AtomicBool=AtomicBool::new(false);
static HIT:AtomicBool=AtomicBool::new(false);
static LEFT:AtomicUsize=AtomicUsize::new(0);
unsafe impl GlobalAlloc for Alloc{
 unsafe fn alloc(&self,l:Layout)->*mut u8{
  if ARMED.load(Ordering::Relaxed)&&l.size()>=65536&&LEFT.fetch_sub(1,Ordering::Relaxed)==0{
   HIT.store(true,Ordering::Relaxed);std::ptr::null_mut()
  }else{System.alloc(l)}
 }
 unsafe fn dealloc(&self,p:*mut u8,l:Layout){System.dealloc(p,l)}
}
#[global_allocator]static ALLOC:Alloc=Alloc;
fn arm(){LEFT.store(std::env::var("FAIL_HIT").unwrap().parse().unwrap(),Ordering::Relaxed);ARMED.store(true,Ordering::Relaxed);}
'''

FRAMES=r'''
mod gpu{
 pub mod dma{
  use std::alloc::{GlobalAlloc,Layout,System};
  pub struct DmaBox(Vec<u8>);
  impl DmaBox{
   pub fn try_new(len:usize)->Option<Self>{
    if crate::PAGE_FAIL.load(crate::Ordering::Relaxed){return None;}
    let p=unsafe{System.alloc_zeroed(Layout::array::<u8>(len.max(1)).ok()?)};
    if p.is_null(){return None;}
    Some(Self(unsafe{Vec::from_raw_parts(p,len,len.max(1))}))
   }
   pub fn len(&self)->usize{self.0.len()}
   pub fn as_ref(&self)->&[u8]{&self.0}
   pub fn as_mut(&mut self)->&mut[u8]{&mut self.0}
  }
 }
 #[path="BUFFER"]pub mod byte_buffer;
}
static PAGE_FAIL:AtomicBool=AtomicBool::new(false);
#[test]fn frame_allocation_failure_preserves_previous_bytes(){
 let mut b=gpu::byte_buffer::Bytes::new(4).unwrap();b.copy_from_slice(&[1,2,3,4]);
 PAGE_FAIL.store(true,Ordering::Relaxed);assert!(b.resize(2560*1600*4).is_err());
 assert_eq!(&*b,&[1,2,3,4]);PAGE_FAIL.store(false,Ordering::Relaxed);
 b.resize(8).unwrap();assert_eq!(&*b,&[1,2,3,4,0,0,0,0]);
 b.remove_prefix(2);assert_eq!(&*b,&[3,4,0,0,0,0]);
}
'''.replace('BUFFER',str(ROOT/'kernel/src/gpu/byte_buffer.rs'))

def run(fixture,test,hits):
    if "extern crate alloc;" not in fixture:fixture="extern crate alloc;\n"+fixture
    priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
    with tempfile.TemporaryDirectory() as tmp:
        p=Path(tmp);(p/'case.rs').write_text(fixture)
        subprocess.run(priority+['rustc','--edition=2021','--crate-name','gpu_abi','--cfg','feature="alloc"','--crate-type','rlib',str(ROOT/'libs/gpu-abi/src/lib.rs'),'-o',str(p/'libgpu_abi.rlib')],check=True)
        subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings','--extern',f'gpu_abi={p/"libgpu_abi.rlib"}',str(p/'case.rs'),'-o',str(p/'case')],check=True)
        for n in hits:
            r=subprocess.run([str(p/'case'),test,'--exact','--nocapture'],env=dict(os.environ,FAIL_HIT=str(n)),stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
            if r.returncode:raise AssertionError(f'large allocation {n} aborted or was accepted:\n{r.stdout}')

class AllocationFailure(unittest.TestCase):
    def test_encoder_and_batch_large_allocations_return_errors(self):
        fixture=(ROOT/'tests/gt710_draw_encode_test.rs').read_text().replace('"../kernel/','"'+str(ROOT/'kernel')+'/')
        fixture+=ALLOC+r'''
trait Checked{fn checked(self)->Result<(),&'static str>;}
impl Checked for (){fn checked(self)->Result<(),&'static str>{Ok(())}}
impl Checked for Result<(),&'static str>{fn checked(self)->Result<(),&'static str>{self}}
#[test]fn large_allocation_is_a_refusal(){
 let mut w=Vec::new();stream::encode::begin(&mut w,1);
 for _ in 0..100{stream::encode::draw(&mut w,pipeline::SOLID,0xff123456,&[[0,0,16,16];100]);}
 arm();let result=encode::translate(w.as_slice(),&View(format::B8G8R8A8)).and_then(|mut f|{f.compact()?;f.batch_draws(1400).checked()?;Ok(f)});
 ARMED.store(false,Ordering::Relaxed);
 if HIT.load(Ordering::Relaxed){assert!(result.is_err(),"allocation failure was not returned to the caller");}
}
'''
        run(fixture,'large_allocation_is_a_refusal',range(18))

    def test_request_and_backlog_storage_are_reserved_before_admission(self):
        source=(ROOT/'kernel/src/gpu/mod.rs').read_text();start=source.index('        let mut reqs: Vec<Request> = Vec::new();',source.index('let chunks = chunks.ok_or(ENOMEM)?;'))
        code=x86(source[start:source.index('        // Test point 27,',start)])
        fixture=ALLOC+r'''
use std::collections::VecDeque;
const ENOMEM:i64=12;type Request=[u8;704];
enum Chunks{Gk208(Vec<()>),Other}
struct Core{backlog:VecDeque<(u64,Request)>}
fn prepare(c:&mut Core,chunks:Chunks)->Result<usize,i64>{CODE
 let _=c.backlog.len();Ok(reqs.capacity())}
#[test]fn native_queue_allocation_is_a_refusal(){
 let mut c=Core{backlog:VecDeque::new()};let _=Chunks::Other;
 arm();let result=prepare(&mut c,Chunks::Gk208(vec![();10000]));ARMED.store(false,Ordering::Relaxed);
 assert!(matches!(result,Err(ENOMEM)),"request/backlog allocation was not refused: {result:?}");assert!(c.backlog.is_empty());
}
'''.replace('CODE',code)
        run(fixture,'native_queue_allocation_is_a_refusal',range(2))

    def test_native_console_readback_uses_frames_when_heap_is_full(self):
        source=item((ROOT/'kernel/src/gpu/mod.rs').read_text(),'pub(crate) fn native_console_readback(')
        code=source[source.index('    let mut first ='):source.index('    let mut last =')]
        fixture=ALLOC+FRAMES+r'''
fn snapshot(bytes:usize)->Result<impl std::ops::Deref<Target=[u8]>,&'static str>{CODE
 first[0]=1;Ok(first)}
#[test]fn console_readback_uses_frames(){
 arm();let first=snapshot(2560*1600*4);ARMED.store(false,Ordering::Relaxed);
 let first=first.unwrap();assert_eq!(first.len(),2560*1600*4);assert_eq!(first[0],1);
}
'''.replace('CODE',code)
        run(fixture,'console_readback_uses_frames',[0])

    def test_diagnostic_span_readbacks_are_fallible(self):
        source=item((ROOT/'kernel/src/gpu/mod.rs').read_text(),'pub(crate) fn native_spans_readback(')
        code=source[source.index('    let mut images ='):source.index('    for (index, poison)')]
        fixture=ALLOC+r'''
fn snapshots(bytes:usize)->Result<[Vec<u8>;2],&'static str>{CODE
 images[0][0]=1;Ok(images)}
#[test]fn span_readback_allocation_is_a_refusal(){
 arm();let result=snapshots(1<<20);ARMED.store(false,Ordering::Relaxed);assert!(result.is_err());
}
'''.replace('CODE',code)
        run(fixture,'span_readback_allocation_is_a_refusal',range(2))

    def test_context_readback_allocations_are_fallible(self):
        source=item((ROOT/'kernel/src/gpu/gk208/ce/readback.rs').read_text(),'pub(in crate::gk208) fn image(')
        code=source[source.index('        let mut snapshots'):source.index('        for (n, poison)')]
        fixture=ALLOC+r'''
fn snapshots(bytes:usize)->Result<[Vec<u8>;2],&'static str>{CODE
 snapshots[0][0]=1;Ok(snapshots)}
#[test]fn readback_allocation_is_a_refusal(){
 arm();let result=snapshots(1<<20);ARMED.store(false,Ordering::Relaxed);
 assert!(result.is_err(),"readback allocation failure was accepted");
}
'''.replace('CODE',code)
        run(fixture,'readback_allocation_is_a_refusal',range(2))

    def test_console_shadow_does_not_require_a_large_heap_allocation(self):
        source=(ROOT/'kernel/src/console/mod.rs').read_text();prepare=item(source,'fn prepare(')
        code=x86(prepare[prepare.index('    let layout = display.layout();')+len('    let layout = display.layout();'):prepare.index('    size_terminals(')]).replace('return;','return None;')
        # The buffer type is taken from the actual retained console state.
        retained=item(source,'struct Attached');typ=retained.split('shadow:',1)[1].split(',',1)[0].strip()
        fixture=ALLOC+r'''
macro_rules! say{($level:ident,$($arg:tt)*)=>{let _=format_args!($($arg)*);}}
const TAG:&str="console";
struct ScreenLayout;impl ScreenLayout{fn bytes(&self)->usize{2560*1600*4}}
TYPE
fn shadow()->Option<Shadow>{let layout=ScreenLayout;CODE
 Some(shadow)}
#[test]fn shadow_uses_system_frames_when_heap_is_full(){
 arm();let shadow=shadow();ARMED.store(false,Ordering::Relaxed);
 let shadow=shadow.expect("the console shadow still needs a 16 MiB heap allocation");assert_eq!(shadow.len(),2560*1600*4);assert!(shadow.iter().all(|&b|b==0));
}
'''.replace('CODE',code).replace('TYPE','type Shadow='+typ+';')
        if typ=='Shadow':
            fixture=fixture.replace('type Shadow=Shadow;',x86(source[source.index('#[cfg(target_arch = "x86_64")]\ntype Shadow'):source.index('/// What `attach` set up:')]))
        if (ROOT/'kernel/src/gpu/byte_buffer.rs').exists():
            fixture+=FRAMES
            run(fixture,'frame_allocation_failure_preserves_previous_bytes',[0])
        run(fixture,'shadow_uses_system_frames_when_heap_is_full',[0])
if __name__=='__main__':unittest.main()
