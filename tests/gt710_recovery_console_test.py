"""Force a splash receipt to overlap the production CE recovery worker.

The worker, canvas hold/drop and receipt are extracted unchanged. The canvas
wait and hardware recovery are barriers, so no scheduling luck is needed.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class RecoveryConsole(unittest.TestCase):
    def test_recovery_waits_for_receipt_and_restores_before_resuming_canvas(self):
        process = (ROOT / 'kernel/src/gpu/gk208/ce/process.rs').read_text()
        recovery = item(process, 'fn recovery_read(')
        worker = item(recovery, 'kthread_run(|| {').split('|| {', 1)[1][:-1]
        receipt = item(process, 'pub(crate) fn console_receipt(')
        # Flatten only the production ce::process namespace in the fixture.
        worker = worker.replace('super::recovery::', 'recovery::')
        receipt = receipt.replace('super::super::native::', 'super::native::')
        console = (ROOT / 'kernel/src/console/mod.rs').read_text()
        hold = item(console, 'pub fn hold_canvas(')
        release = item(console, 'impl Drop for CanvasHold')
        harness = r'''
use std::sync::{Barrier, Mutex as StdMutex, MutexGuard, OnceLock, mpsc};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
struct Mutex<T>(StdMutex<T>);
impl<T> Mutex<T> {
    const fn new(value:T)->Self {Self(StdMutex::new(value))}
    fn lock(&self)->MutexGuard<'_,T> {self.0.lock().unwrap()}
}
static EVENT:OnceLock<mpsc::Sender<&str>>=OnceLock::new();
static RESUME:Barrier=Barrier::new(2);
static FAIL:AtomicBool=AtomicBool::new(false);
fn pause(event:&'static str) {EVENT.get().unwrap().send(event).unwrap();RESUME.wait();}
mod console {
    use super::*;
    pub static CANVAS_HOLDS:AtomicUsize=AtomicUsize::new(0);
    static PRESENTING:AtomicBool=AtomicBool::new(true);
    static REDRAW:AtomicBool=AtomicBool::new(false);
    pub struct CanvasHold;
    // The production wait blocks until the presenting pass has ended.
    fn wait_until_not_presenting() {
        assert!(PRESENTING.load(Ordering::SeqCst));
        pause("canvas-wait");
        assert!(!PRESENTING.load(Ordering::SeqCst));
    }
    // HOLD
    // RELEASE
    pub fn end_pass(){PRESENTING.store(false,Ordering::SeqCst);}
    pub fn repainted()->bool{REDRAW.load(Ordering::Acquire)}
}
mod gk208 {
    #[derive(Clone,Copy)] pub struct ConsoleSurface {pub vram:u64}
    pub mod memory {
        #[derive(Clone,Copy)] pub struct Range {pub start:u64}
        pub struct Object {pub range:Range}
        pub struct Layout {pub objects:Vec<Object>}
    }
    pub mod native {
        pub struct Evo;
        pub fn console_receipt(_: &Evo, regs:&mut super::ce::Registers,
            surface:super::ConsoleSurface, allocation:super::memory::Range)
            ->Result<u64,&'static str> {
            assert_eq!(regs.0,7);assert_eq!(surface.vram,allocation.start);Ok(41)
        }
    }
    pub mod ce {
        use crate::*;
        pub struct Registers(pub usize);
        pub struct Session {
            pub layout:super::memory::Layout,pub registers:usize,
            pub console:super::ConsoleSurface,pub evo:super::native::Evo,pub work:Option<()>
        }
        static SESSION:Mutex<Option<Session>>=Mutex::new(None);
        static RECOVERY_RESULT:Mutex<Option<Result<String,&'static str>>>=Mutex::new(None);
        static RECOVERY_READY:AtomicBool=AtomicBool::new(false);
        struct Waiters;
        impl Waiters {fn wake_up(&self){assert!(RECOVERY_READY.load(Ordering::Acquire));}}
        static RECOVERY_WAITERS:Waiters=Waiters;
        mod recovery {
            pub fn probe(_: &mut super::Session)->Result<String,&'static str> {
                // Old code reaches hardware while the splash pass is live.
                if crate::console::CANVAS_HOLDS.load(super::Ordering::SeqCst)==0 {
                    crate::pause("recovery-without-canvas");
                }
                assert!(super::SESSION.lock().is_none());
                if crate::FAIL.load(super::Ordering::Acquire) {Err("recovery fault")}
                else {Ok(String::from("recovered"))}
            }
        }
        fn worker(){WORKER}
        RECEIPT
        pub fn exercise() {
            *SESSION.lock()=Some(Session {
                layout:super::memory::Layout {objects:vec![super::memory::Object {
                    range:super::memory::Range{start:0x1000}}]},
                registers:7,console:super::ConsoleSurface{vram:0x1000},
                evo:super::native::Evo,work:None
            });
            let (tx,rx)=mpsc::channel();EVENT.set(tx).unwrap();
            let thread=std::thread::spawn(worker);
            // Either the fixed worker waits for the pass, or the old worker
            // has already removed SESSION and entered recovery hardware.
            let event=rx.recv().unwrap();
            let observed=console_receipt();
            crate::console::end_pass();RESUME.wait();thread.join().unwrap();
            assert_eq!(observed,Ok(41),"in-progress splash lost its native session");
            assert_eq!(event,"canvas-wait");
            assert!(SESSION.lock().is_some());
            assert_eq!(crate::console::CANVAS_HOLDS.load(Ordering::SeqCst),0);
            assert!(crate::console::repainted());
            assert_eq!(*RECOVERY_RESULT.lock(),Some(if FAIL.load(Ordering::Acquire) {
                Err("recovery fault")
            } else {Ok(String::from("recovered"))}));
        }
    }
}
fn main(){
    let _=console::hold_canvas as fn()->console::CanvasHold;
    FAIL.store(std::env::args().nth(1).unwrap()=="fail",Ordering::Release);
    gk208::ce::exercise();
}
'''.replace('WORKER', worker).replace('RECEIPT', receipt)
        harness = harness.replace('// HOLD', hold).replace('// RELEASE', release)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'recovery.rs').write_text(harness)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                           str(path / 'recovery.rs'), '-o', str(path / 'recovery')], check=True)
            for result in ['pass', 'fail']:
                subprocess.run([str(path / 'recovery'), result], check=True, timeout=10)


if __name__ == '__main__':
    unittest.main()
