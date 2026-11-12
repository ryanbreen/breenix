"""Force the cold pipeline work on the retained compositor startup path.

Execute the production worker's eligibility/selection and completion accounting.
Device answers are forced, so this counts renderer work without a timing oracle
or a GPU. The retained capture ties this work to font/target resource waits.
"""
from pathlib import Path
import unittest

from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class StartupPipelines(unittest.TestCase):
    def test_cold_start_prepares_only_the_renderers_operations(self):
        source = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        worker = item(source, 'fn warm_next(')
        eligibility = next(line.strip() for line in worker.splitlines()
                           if line.strip().startswith('let warming ='))
        selection = next(line.strip() for line in worker.splitlines()
                         if line.strip().startswith('let pipeline ='))
        startup = (ROOT / 'userspace/programs/src/bwm/startup_prepare.rs').read_text()
        # Before the explicit requirement-set API, both callers request the
        # default catalogue. Keep that old path executable to reproduce it.
        requirements = '\n'.join(line for line in startup.splitlines()
                                 if line.startswith('pub const ') and 'PIPELINES' in line)
        if not requirements:
            requirements = ('const SCENE_PIPELINES:u32=0xffff;\n'
                            'const CHROME_PIPELINES:u32=0xffff;')
        run_rust(self, r'''
use std::collections::BTreeMap;
mod abi { pub mod pipeline {
 pub const SOLID:u32=0; pub const COPY:u32=1; pub const NEAREST_SCALED:u32=3;
 pub const COVERAGE:u32=7; pub const COVERAGE_SPANS:u32=8; pub const BITMAP_TEXT:u32=9;
}}
use abi::pipeline;
mod encode { pub const ALL_WARM:u32=0xffff; }
#[derive(PartialEq)] enum CtxState { Live, Quarantined }
struct Ctx { owner:u64, state:CtxState, device_alive:bool, dropped:bool,
 warm_catalogue:bool, warm_required:u32, warm:u32, warm_after:u64,
 warm_failures:u32, native_warm_scratch:Option<()> }
struct WarmStep { ctx:u32, pipeline:u32, left:u32, refused:bool, lost:bool }
struct Core { contexts:BTreeMap<u32,Ctx>, warm_step:Option<WarmStep> }
fn now_ns()->u64 { 1_000_000_000 }
fn notify() {}
@MASK@
impl Core { fn settle(&mut self,_:u32) {} @RETURNED@ }
@REQUIREMENTS@
fn cold(required:u32, base:u32)->Vec<u32> {
 let mut c=Core { contexts:BTreeMap::from([(1,Ctx { owner:1,state:CtxState::Live,
  device_alive:true,dropped:false,warm_catalogue:required!=0,warm_required:required,
  warm:base,warm_after:0,warm_failures:0,native_warm_scratch:Some(()) })]),warm_step:None };
 @ELIGIBILITY@
 let mut work=Vec::new();
 while warming(&c.contexts[&1]) {
  let ctx=&c.contexts[&1];
  @SELECTION@
  work.push(pipeline);
  c.warm_step=Some(WarmStep { ctx:1,pipeline,left:2,refused:false,lost:false });
  c.warm_returned(Some(true));
  assert_eq!(c.contexts[&1].warm & (1<<pipeline),0,"setup answer is not draw completion");
  c.warm_returned(Some(true));
  assert_ne!(c.contexts[&1].warm & (1<<pipeline),0);
 }
 assert_eq!(c.contexts[&1].warm & required,required);
 // This field is also consumed by the post-fix worker; read it in the old fixture.
 assert_eq!(c.contexts[&1].warm_required,required);
 if required!=0 { assert!(c.contexts[&1].native_warm_scratch.is_none()); }
 work
}
#[test] fn startup_and_later_chrome_have_no_unused_cold_programs() {
 let scene=cold(SCENE_PIPELINES,0); let chrome=cold(CHROME_PIPELINES,0);
 println!("cold scene={scene:?} chrome={chrome:?}; realizations={}",scene.len()+chrome.len());
 // All later window titles, controls and clock digits use these same paths;
 // bitmap fallback is included even when TrueType loaded successfully.
 let scene_used=[pipeline::SOLID,pipeline::COPY];
 let chrome_used=[pipeline::SOLID,pipeline::COPY,pipeline::NEAREST_SCALED,
  pipeline::COVERAGE,pipeline::COVERAGE_SPANS,pipeline::BITMAP_TEXT];
 for pipe in scene_used { assert!(scene.contains(&pipe)); }
 for pipe in chrome_used { assert!(chrome.contains(&pipe)); }
 assert_eq!(scene.len()+chrome.len(),8,"startup still prepares unused renderer programs");
 assert_eq!(cold(catalogue_mask(),0).len(),16,"general-purpose preparation changed");
 assert!(cold(0,0).is_empty());
 // Native fixed setup has already completed SOLID; it must not warm it twice.
 assert_eq!(cold(SCENE_PIPELINES,1<<pipeline::SOLID),vec![pipeline::COPY]);
}
'''.replace('@MASK@', item(source, 'fn catalogue_mask('))
           .replace('@RETURNED@', item(source, '    fn warm_returned(')
                    .replace('#[cfg(target_arch = "x86_64")]', ''))
           .replace('@REQUIREMENTS@', requirements)
           .replace('@ELIGIBILITY@', eligibility).replace('@SELECTION@', selection))


if __name__ == '__main__':
    unittest.main()
