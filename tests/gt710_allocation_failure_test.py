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

def run(fixture,test,hits):
    priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
    with tempfile.TemporaryDirectory() as tmp:
        p=Path(tmp);(p/'case.rs').write_text(fixture)
        subprocess.run(priority+['rustc','--edition=2021','--crate-name','gpu_abi','--crate-type','rlib',str(ROOT/'libs/gpu-abi/src/lib.rs'),'-o',str(p/'libgpu_abi.rlib')],check=True)
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

    def test_console_shadow_does_not_require_a_large_heap_allocation(self):
        source=(ROOT/'kernel/src/console/mod.rs').read_text();prepare=item(source,'fn prepare(')
        code=x86(prepare[prepare.index('    let mut shadow =') if '    let mut shadow =' in prepare else prepare.index('    #[cfg(target_arch = "x86_64")]\n    let'):prepare.index('    size_terminals(')]).replace('return;','return None;')
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
        run(fixture,'shadow_uses_system_frames_when_heap_is_full',[0])
if __name__=='__main__':unittest.main()
