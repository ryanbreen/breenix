"""Final text-panel reasons belong to the runner's pre-case storage boundary."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class PanelStorage(unittest.TestCase):
    def test_failed_panel_preserves_reasons_without_post_baseline_pages(self):
        source = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        kinds = source[source.index('pub(super) enum Outcome {'):source.index('/// Print a settled memory sample')]
        helpers = '\n'.join(item(source, 'fn ' + name + '(') for name in
                            ('panel_reason_storage', 'retain_panel_reason') if 'fn ' + name + '(' in source)
        setup = next((line.strip() for line in source.splitlines() if 'let mut panel_reasons =' in line), '')
        retention = next((line.strip() for line in source.splitlines() if 'retain_panel_reason(&mut states' in line), '')
        fixture = r'''
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};
static PAGES:AtomicIsize=AtomicIsize::new(0);
struct PageCount;
unsafe impl GlobalAlloc for PageCount {
 unsafe fn alloc(&self,l:Layout)->*mut u8 {let p=System.alloc(l);if !p.is_null(){PAGES.fetch_add(l.size().div_ceil(4096) as isize,Ordering::Relaxed);}p}
 unsafe fn dealloc(&self,p:*mut u8,l:Layout){PAGES.fetch_sub(l.size().div_ceil(4096) as isize,Ordering::Relaxed);System.dealloc(p,l);}
}
#[global_allocator]static ALLOC:PageCount=PageCount;
const MSG_MAX:usize=160;
KINDS
HELPERS
#[test]fn retained_panel() {
 let mut states:Vec<Vec<State>>=vec![(0..82).map(|_|State::Pending).collect()];
 let textual=true;
 SETUP
 let before=PAGES.load(Ordering::Relaxed);
 for k in 0..82 {
  states[0][k]=State::Running;
  states[0][k]=State::Done(if k%10==0 {Outcome::Pass{ms:3}} else if k%10==1 {Outcome::Skip{msg:"pending target".into()}} else {Outcome::Fail{ms:7,msg:"exact failure reason including UTF-8: café".into()}});
  let c=0;
  RETENTION
 }
 let growth=PAGES.load(Ordering::Relaxed)-before;
 assert!(growth<=48,"retained panel reasons grew by {growth} pages");
 for(k,s)in states[0].iter().enumerate(){match s {
  State::Done(Outcome::Pass{ms})=>{assert_eq!(k%10,0);assert_eq!(*ms,3)},
  State::Done(Outcome::Skip{msg})=>{assert_eq!(k%10,1);assert_eq!(msg,"pending target")},
  State::Done(Outcome::Fail{ms,msg})=>{assert_eq!(*ms,7);assert_eq!(msg,"exact failure reason including UTF-8: café")},
  _=>panic!("outcome lost")
 }}
 let _=(MSG_MAX,textual);
}
'''.replace('KINDS', kinds.replace('pub(super)', 'pub')).replace('HELPERS', helpers).replace('SETUP', setup).replace('RETENTION', retention)
        # Before the repair the production retention call is absent.
        if not retention:
            fixture = fixture.replace('  let c=0;','')
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p/'check.rs').write_text(fixture)
            command = ['rustc', '--edition=2021', '--test', '-Dwarnings', str(p/'check.rs'), '-o', str(p/'check')]
            if sys.platform == 'darwin': command = ['taskpolicy', '-b', *command]
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = subprocess.run([str(p/'check')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__': unittest.main()
