"""Execute the runner's result publication with page-granular allocations."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class OutcomeMemory(unittest.TestCase):
    def test_serial_and_panels_keep_results_without_unused_message_pages(self):
        source = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        start = source.index('                match &outcome { Outcome::Pass')
        end = source.index('                done += 1;', start)
        publication = source[start:end]
        kinds = source[source.index('pub(super) enum Outcome {'):source.index('/// Print a settled memory sample')]
        fixture = r'''
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
static PAGES: AtomicUsize = AtomicUsize::new(0);
struct PageCount;
unsafe impl GlobalAlloc for PageCount {
    unsafe fn alloc(&self, l:Layout)->*mut u8 {
        let p=System.alloc(l); if !p.is_null(){PAGES.fetch_add(l.size().div_ceil(4096),Ordering::Relaxed);} p
    }
    unsafe fn dealloc(&self,p:*mut u8,l:Layout){PAGES.fetch_sub(l.size().div_ceil(4096),Ordering::Relaxed);System.dealloc(p,l);}
}
#[global_allocator] static ALLOC:PageCount=PageCount;
KINDS
#[derive(Default)] struct Counts {passed:usize,failed:usize,skipped:usize}
struct Screen {fb:Option<()>}
struct Suite {gui:bool,defer_display:bool}
const REASON:&str="missing the GPU core with a device admitted to the profile (G1): CAPS status 1 (no GPU core backend)";
static EMITTED:AtomicUsize=AtomicUsize::new(0);
fn emit(line:&str){assert!(line.ends_with(REASON));EMITTED.fetch_add(1,Ordering::Relaxed);}
impl Suite {
 fn exercise(&self,fb:bool) {
    let screen=Screen{fb:fb.then_some(())};
    let mut states:Vec<Vec<State>>=[12,22,19,19,22,26,25,16,9].into_iter().map(|n|(0..n).map(|_|State::Pending).collect()).collect();
    let mut counts=Counts::default();
    let before=PAGES.load(Ordering::Relaxed);
    for c in 0..states.len(){for k in 0..states[c].len(){
        states[c][k]=State::Running;
        let mut outcome=Outcome::Fail{ms:7,msg:REASON.into()};
        assert!(matches!(&mut outcome,Outcome::Fail{ms:7,..}));
        let line=format!("SUITE gpu CASE {c}/{k} FAIL ms=7 msg={REASON}");
PUBLICATION
    }}
    let growth=PAGES.load(Ordering::Relaxed)-before;
    assert_eq!((counts.passed,counts.failed,counts.skipped),(0,170,0));
    let textual=self.gui||screen.fb.is_some()||self.defer_display;
    if !textual {assert!(growth<=48,"unused messages retained {growth} pages")}
    for state in states.iter().flatten(){match state {
        State::Done(Outcome::Fail{ms,msg})=>{assert_eq!(*ms,7);if textual{assert_eq!(msg,REASON)}},
        _=>panic!("failed outcome lost")
    }}
    if textual {assert!(growth>=170,"text panel must retain its reasons")}
 }
}
#[test] fn serial_gpu_and_text_panels() {
    // GPU tiles consume status only; CPU, GUI and deferred text panels keep reasons.
    for (gui,defer_display,fb) in [(false,false,false),(false,false,true),(true,false,false),(false,true,false)] {
        Suite{gui,defer_display}.exercise(fb);
    }
    assert_eq!(EMITTED.load(Ordering::Relaxed),680);
    // Exercise every outcome kind too, so the extracted production enum stays honest.
    assert!(matches!(Outcome::Pass{ms:1},Outcome::Pass{ms:1}));
    assert!(matches!(Outcome::Skip{msg:String::new()},Outcome::Skip{..}));
}
'''.replace('KINDS', kinds.replace('pub(super)', 'pub')).replace('PUBLICATION', publication)
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            (out / 'fixture.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(out / 'fixture.rs'), '-o', str(out / 'fixture')], check=True)
            subprocess.run([str(out / 'fixture')], check=True)


if __name__ == '__main__':
    unittest.main()
