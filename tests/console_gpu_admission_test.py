"""Exercise kernel context ownership, pass-boundary routing and failure reporting."""
from pathlib import Path
import subprocess,sys,tempfile,unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item

def run(source):
    prefix=['taskpolicy','-b'] if sys.platform=='darwin' else []
    with tempfile.TemporaryDirectory() as tmp:
        p=Path(tmp);(p/'check.rs').write_text(source)
        subprocess.run(prefix+['rustc','--edition=2021','--test','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
        subprocess.run([str(p/'check'),'--nocapture','--test-threads=1'],check=True)

def x86(source):
    return source.replace('#[cfg(target_arch = "x86_64")]','').replace('#[cfg(target_arch = "aarch64")]','#[cfg(any())]').replace('cfg!(target_arch = "x86_64")','true')

class Admission(unittest.TestCase):
    def test_kernel_contexts_do_not_warm_client_catalogues(self):
        core=(ROOT/'kernel/src/gpu/mod.rs').read_text()
        self.assertTrue('fn create_kernel_context(' in core,'kernel contexts still enter client warm-up')
        for path,marker in [('screen.rs','fn new()'),('console.rs','pub(super) fn new()')]:
            constructor=item((ROOT/'kernel/src/gpu'/path).read_text(),marker)
            self.assertIn('create_kernel_context()',constructor,path)
        helper=x86(item(core,'fn create_kernel_context('))
        run("""
fn create_context_with_catalogue(owner:u64,warm:bool)->Result<(u64,u32),i64>{
 assert_eq!(owner,0);assert!(!warm,"kernel context charged client catalogue scratch");Ok((7,0))
}
HELPER
#[test]fn screen_and_panic_contexts_need_no_process_record(){
 assert_eq!(create_kernel_context(),Ok((7,0)));assert_eq!(create_kernel_context(),Ok((7,0)));
}
""".replace('HELPER',helper))

    def test_backend_changes_only_at_a_pass_boundary_and_layout_is_opaque(self):
        source=(ROOT/'kernel/src/console/display/linear.rs').read_text()
        self.assertTrue('pub fn begin_pass(' in source,'linear backend is selected independently for each row')
        functions=x86('\n'.join(item(source,m) for m in ('pub fn uses_gpu(','pub fn begin_pass(','pub fn layout(')))
        kind=x86(item((ROOT/'kernel/src/console/display/mod.rs').read_text(),'pub fn kind('))
        run("""
use std::sync::{OnceLock,atomic::{AtomicBool,Ordering}};
static ADMITTED:AtomicBool=AtomicBool::new(false);
mod gpu {pub fn display_active()->bool{crate::ADMITTED.load(crate::Ordering::Acquire)}pub mod console {pub fn active()->bool{false}}}
mod raster {pub struct Font;impl Font{pub fn for_display(_:usize,_:usize)->Self{Self}}}
struct Layout{width:usize,height:usize,stride_bytes:usize,bytes_per_pixel:usize,reserved:u8,bgr:bool,font:raster::Font}
struct Surface{width:usize,height:usize,pitch:usize}
struct Linear{native:OnceLock<Surface>,gpu_active:AtomicBool,layout:Layout}
impl Linear{FUNCTIONS}
enum Display{Linear(Linear)}
impl Display{KIND}
#[test]fn read_source_is_stable_until_the_next_pass(){
 let native=OnceLock::new();assert!(native.set(Surface{width:8,height:4,pitch:32}).is_ok());
 let l=Linear{native,gpu_active:AtomicBool::new(false),layout:Layout{width:1,height:1,stride_bytes:4,bytes_per_pixel:4,reserved:0,bgr:true,font:raster::Font}};
 assert!(!l.uses_gpu());ADMITTED.store(true,Ordering::Release);
 assert!(!l.uses_gpu(),"admission changed the source during a check");
 assert!(l.begin_pass());assert!(l.uses_gpu());assert!(!l.begin_pass());
 ADMITTED.store(false,Ordering::Release);assert!(l.uses_gpu(),"GPU loss resumed CPU BAR writes");
 let layout=l.layout();assert_eq!((layout.width,layout.height,layout.stride_bytes,layout.bytes_per_pixel,layout.reserved,layout.bgr),(8,4,32,4,255,true));let _=layout.font;
 assert_eq!(Display::Linear(l).kind(),"GPU core scanout target");
}
""".replace('FUNCTIONS',functions).replace('KIND',kind))
        render=item((ROOT/'kernel/src/console/mod.rs').read_text(),'fn render_thread(')
        self.assertIn('display.begin_pass()',render)
        self.assertLess(render.index('display.begin_pass()'),render.index('screen.update('))

    def test_modes_never_advertise_an_unregistered_native_console(self):
        source=(ROOT/'kernel/src/gpu/console.rs').read_text()
        describe=x86(item(source,'pub(super) fn describe(')).replace('pub(super) fn describe','pub(crate) fn describe')
        run("""
use std::sync::atomic::{AtomicU64,AtomicBool,Ordering};
static FIXTURE:AtomicU64=AtomicU64::new(0);static HANDOFF:AtomicBool=AtomicBool::new(false);static HASH:AtomicU64=AtomicU64::new(0);
fn active()->bool{true}fn caller()->Result<u64,()>{Err(())}fn firmware_hash()->u64{0}
mod display{pub fn owner(_:u64)->bool{false}}
mod arch{pub fn firmware_writes()->u64{0}}
mod gk208{pub fn console_state()->Option<((),u64,bool,u64)>{Some(((),u32::MAX as u64,true,0))}}
mod abi{#[derive(Default)]pub struct Modes{pub console_resource:u64,pub console_draws:u64,pub console_fixture_bytes:u64,pub handoff:u64,pub firmware_hash_at_handoff:u64,pub firmware_hash_now:u64,pub firmware_writes_after_handoff:u64,pub scanout_resource:u64,pub binding_known:u32}}
mod core {pub mod screen{pub fn describe()->(u64,u64){(0,0)}}pub mod console{use crate::*;DESCRIBE}}
#[test]fn an_early_native_surface_is_not_a_shared_readback_binding(){
 let mut modes=abi::Modes::default();core::console::describe(&mut modes);
 assert_eq!((modes.console_resource,modes.scanout_resource,modes.binding_known),(0,0,0));
}
""".replace('DESCRIBE',describe))
        core=(ROOT/'kernel/src/gpu/mod.rs').read_text()
        self.assertIn('screen::await_binding()',item(core,'pub fn take_display('))
        self.assertIn('screen::await_binding()',item(core,'fn read_back('))

    def test_update_errors_and_decisive_case_lines_survive(self):
        console=(ROOT/'kernel/src/console/mod.rs').read_text()
        self.assertTrue('last_failure' in item(console,'    fn update('),'update failure is discarded')
        self.assertIn('console last failure:',item(console,'pub fn proc_console_check('))
        render=item(console,'fn render_thread(')
        self.assertIn('screen.last_failure.is_some()',render,'a never-completed frame strands proc checks')

    def test_capped_messages_keep_the_decisive_check(self):
        suite=(ROOT/'userspace/programs/src/suite_console.rs').read_text()
        helper=item(suite,'fn check_failure(') if 'fn check_failure(' in suite else ''
        identical=item(suite,'fn identical(')
        run("""
type Verdict=Result<(),String>;
fn require(ok:bool,text:&str)->Verdict{if ok{Ok(())}else{Err(text.into())}}
HELPER
IDENTICAL
#[test]fn capped_failure_starts_with_the_check(){
 let text=format!("console foreground: tty1\nconsole display: {}\nconsole scanout: not tied to the screen: target refused\nconsole frame: 0 incomplete\nconsole check: shadow and screen not compared\nconsole last failure: target refused\n","x".repeat(300));
 let error=identical(&text).unwrap_err();assert!(error.starts_with("console check:"),"{error}");
 assert!(error[..error.len().min(200)].contains("target refused"),"{error}");
}
""".replace('HELPER',helper).replace('IDENTICAL',identical))

    def test_startup_samples_are_collected_after_the_console_check(self):
        helper=(ROOT/'userspace/programs/src/gt710_artifacts.rs').read_text()
        self.assertLess(helper.index('read_to_string("/proc/console/check")'),helper.index('read("/proc/gt710-pixels")'))

if __name__=='__main__':unittest.main()
