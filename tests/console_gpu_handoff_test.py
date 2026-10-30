"""Admission must repaint CPU rows and preserve shared GPU scanout bindings."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class Handoff(unittest.TestCase):
    def test_publication_repaints_and_native_cpu_receipt_preserves_gpu_binding(self):
        source = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        install = item(source, 'pub(crate) fn install_native(')
        tail = install[install.index('    if result.is_ok()'):install.rfind('}')]
        console = (ROOT / 'kernel/src/gpu/console.rs').read_text()
        receipt = item(console, 'pub(crate) fn native_repainted(')
        fixture = r'''
use std::sync::atomic::{AtomicBool,Ordering};
static REDRAW:AtomicBool=AtomicBool::new(false);
static CLEARED:AtomicBool=AtomicBool::new(false);
struct Display { stack:Vec<u64>, changing:bool,outstanding:u32,binding:Option<u64>,holds:Vec<u64> }
struct Core {display:Display}
impl Core {fn native_console_restored(&mut self){CLEARED.store(true,Ordering::Release);}}
fn with_core<T>(f:impl FnOnce(&mut Core)->T)->Option<T>{Some(f(&mut Core{display:Display{stack:vec![],changing:false,outstanding:0,binding:Some(42),holds:vec![42]}}))}
fn notify(){}
mod compose{pub fn register(){}}
mod display{pub fn active()->bool{true}}
mod console{pub fn request_repaint(){crate::REDRAW.store(true,crate::Ordering::Release);}}
mod gk208{pub fn console_state()->Option<((),(),bool,())>{Some(((),(),true,()))}}
fn install(result:Result<(),&'static str>)->Result<(),&'static str>{TAIL}
RECEIPT
#[test] fn admitted_console_repaints_unchanged_cpu_rows(){
 assert!(display::active());let _ = console::request_repaint as fn();
 assert_eq!(install(Err("refused")),Err("refused"));assert!(!REDRAW.load(Ordering::Acquire));
 install(Ok(())).unwrap();assert!(REDRAW.load(Ordering::Acquire),"CPU row cache survived GPU admission");
}
#[test] fn gpu_repaint_keeps_the_shared_binding(){native_repainted();assert!(!CLEARED.load(Ordering::Acquire),"native CPU receipt erased the GPU binding");}
'''.replace('TAIL', tail).replace('RECEIPT', receipt.replace('pub(crate)', ''))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'handoff.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                str(path / 'handoff.rs'), '-o', str(path / 'handoff')], check=True)
            subprocess.run([str(path / 'handoff'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
