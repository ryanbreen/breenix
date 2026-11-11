"""A paused ordinary UART record must finish without leaving an abandoned prefix."""
from pathlib import Path
import os,subprocess,tempfile,unittest
ROOT=Path(__file__).resolve().parents[1]
class NormalOrder(unittest.TestCase):
 def test_stalled_request_and_exit_are_each_one_complete_line(self):
  source=(ROOT/'kernel/src/uart_owner.rs').read_text()
  normal='Ownership::acquire_ordered()' if 'pub fn acquire_ordered()' in source else 'Ownership::acquire()'
  harness=(ROOT/'tests/uart_owner.rs').read_text()
  for name in ['stage_lines','uart_owner']:
   harness=harness.replace('../kernel/src/'+name+'.rs',str(ROOT/'kernel/src'/(name+'.rs')))
  harness+=r'''
static ORDER_STATE:State=State::new();
static ORDER_STALLED:AtomicBool=AtomicBool::new(false);
static ORDER_RELEASE:AtomicBool=AtomicBool::new(false);
static ORDER_TRIES:AtomicUsize=AtomicUsize::new(0);
static ORDER_TICKS:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
enum OrderedTest{}
impl Port for OrderedTest{
 type Writer=u8;
 fn state()->&'static State{&ORDER_STATE}
 fn cpu_tag()->u32{Test::cpu_tag()}
 fn counter()->u64{ORDER_TICKS.fetch_add(1_000_000,Ordering::Relaxed)}
 fn ticks(ns:u64)->u64{Test::ticks(ns)}
 fn mask_interrupts()->u64{if SECOND.with(Cell::get){ORDER_TRIES.fetch_add(1,Ordering::Relaxed);}0}
 fn restore_interrupts(_:u64){}
 fn ready()->bool{
  let n=READIED.with(|n|{n.set(n.get()+1);n.get()});
  if !SECOND.with(Cell::get)&&n==12{
   ORDER_STALLED.store(true,Ordering::Release);
   wait_for(||ORDER_RELEASE.load(Ordering::Acquire));
  }
  true
 }
 fn write(byte:u8,owner:&Ownership<Self>){let _=owner;WIRE.lock().unwrap().push(byte);if SECOND.with(Cell::get){SECOND_WROTE.fetch_add(1,Ordering::Release);}}
 fn capture(byte:u8){Test::capture(byte)}
 fn lead(writer:u8,unit:&[u8])->&'static[u8]{Test::lead(writer,unit)}
 fn line_open()->bool{Test::line_open()}
 fn wrote(writer:u8,byte:u8){Test::wrote(writer,byte)}
 fn markers()->&'static[&'static str]{Test::markers()}
}
fn normal()->Ownership<OrderedTest>{@NORMAL@}
#[test]fn ordinary_uart_wait_does_not_abandon_the_hostpointer_prefix(){
 let _one=SERIAL.lock().unwrap_or_else(|e|e.into_inner());WIRE.lock().unwrap().clear();OPEN.store(0,Ordering::Relaxed);SECOND_WROTE.store(0,Ordering::Relaxed);
 let request=b"SUITE window-manager HOSTPOINTER focus/ctrl-c seq=409 x=84 y=164 width=2560 height=1600 buttons=0 wheel=0\n";
 let exit=b"[signal] Process 381 terminated by signal 9\n";
 let one=thread::spawn(move||uart_owner::write::<OrderedTest,false>(1,request,Kind::Line,&mut normal()));
 wait_for(||ORDER_STALLED.load(Ordering::Acquire));
 let two=thread::spawn(move||{SECOND.with(|s|s.set(true));uart_owner::write::<OrderedTest,false>(2,exit,Kind::Line,&mut normal());});
 // Force the competing writer beyond the old takeover budget without a wall-clock race.
 wait_for(||ORDER_TRIES.load(Ordering::Acquire)>=1000||SECOND_WROTE.load(Ordering::Acquire)>0);
 ORDER_RELEASE.store(true,Ordering::Release);one.join().unwrap();two.join().unwrap();
 let mut expected=request.to_vec();expected.extend_from_slice(exit);
 assert_eq!(*WIRE.lock().unwrap(),expected,"a normal exit split the request and left a malformed prefix before replay");
}
'''.replace('@NORMAL@',normal)
  with tempfile.TemporaryDirectory(prefix='serial-normal-') as directory:
   path=Path(directory);(path/'check.rs').write_text(harness)
   subprocess.run(['taskpolicy','-b',os.environ.get('RUSTC','rustc'),'--edition=2021','--test','-Dwarnings',str(path/'check.rs'),'-o',str(path/'check')],check=True)
   subprocess.run([str(path/'check'),'--exact','ordinary_uart_wait_does_not_abandon_the_hostpointer_prefix'],check=True)
if __name__=='__main__':unittest.main()
