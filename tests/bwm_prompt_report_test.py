"""Drive production deferred validation with controlled composed generations."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class PromptReport(unittest.TestCase):
    def test_deferred_prompt_reporting(self):
        old = os.environ.get('UITEST_OLD_COMMIT')

        def source(path):
            return subprocess.check_output(['git', 'show', f'{old}:{path}'], cwd=ROOT, text=True) if old else (ROOT / path).read_text()

        chrome = source('userspace/programs/src/bwm/chrome.rs')
        deferred = chrome[chrome.index('pub(super) fn run_deferred_check('):chrome.index('// ─── Drawing Helpers')]
        # Supply the new session argument to the historical function without
        # altering its control flow, so the baseline fails on missing evidence.
        if 'strict_startup: bool' not in deferred:
            deferred = deferred.replace('    resizing: bool,', '    resizing: bool,\n    _strict_startup: bool,')
        config = source('userspace/programs/src/bwm/config.rs')
        main = source('userspace/programs/src/bwm.rs')
        start = main.index('    if !strict_startup {\n        for win in &windows')
        request = item(main[start:], 'if !strict_startup {')
        terminal = subprocess.check_output(['git', 'show', f"{os.environ['BTERM_OLD_COMMIT']}:userspace/programs/src/bterm.rs"], cwd=ROOT, text=True) if 'BTERM_OLD_COMMIT' in os.environ else source('userspace/programs/src/bterm.rs')
        redraw = next(line.strip() for line in terminal.splitlines() if line.strip().startswith('let need_redraw ='))
        watch = ROOT / 'userspace/programs/src/bwm/chrome/prompt_watch.rs'
        historical_watch = subprocess.run(['git', 'show', f'{old}:userspace/programs/src/bwm/chrome/prompt_watch.rs'], cwd=ROOT, capture_output=True, text=True) if old else None
        watcher = historical_watch.stdout if historical_watch and historical_watch.returncode == 0 else watch.read_text() if not old and watch.exists() else '#[derive(Default)] pub struct Watchers; impl Watchers { pub fn retain(&mut self,_:impl FnMut(u64)->bool){} }'

        with tempfile.TemporaryDirectory(prefix='bwm-prompt-') as directory:
            temp = Path(directory)
            (temp / 'Cargo.toml').write_text('[package]\nname="bwm-prompt-check"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\nserde_json="=1.0.154"\n')
            (temp / 'src').mkdir()
            (temp / 'validation.rs').write_text(source('userspace/programs/src/bwm/chrome/validation.rs'))
            (temp / 'frame_text.rs').write_text(source('userspace/programs/src/bwm/chrome/frame_text.rs'))
            (temp / 'prompt_watch.rs').write_text(watcher)

            program = r'''
extern crate alloc;
extern crate self as libbui;
#[path="@ROOT@/libs/libbui/src/tree.rs"] pub mod tree;
#[path="../validation.rs"] mod validation;
#[path="../frame_text.rs"] mod frame_text;
use validation::ValidationQueue;
#[path="@ROOT@/libs/breengel/src/tree_commit.rs"] mod tree_commit;
#[path="../prompt_watch.rs"] mod prompt_watch;
extern crate self as libbreenix;
#[allow(dead_code)] mod window {
 pub mod abi {pub mod op {pub const MAP_TREE:u32=1;} #[derive(Default,Clone,Copy)] pub struct MapTree {pub id:u64,pub addr:u64,pub len:u64}}
 pub fn call(_:u32,m:&mut abi::MapTree)->Result<u64,String> {
  assert!(m.id==7||m.id==8); m.addr=m.id; m.len=4096;
  READERS.with(|r|r.set(r.get()+1));Ok(0)
 }
 use super::READERS;
}
#[allow(dead_code)] mod memory {pub fn munmap(_: *mut u8,_:usize)->Result<(),String>{super::READERS.with(|r|r.set(r.get()-1));Ok(())}}
thread_local!{static READERS:std::cell::Cell<u64>=const{std::cell::Cell::new(0)};}

use tree::{Tree,Role,Node};
#[derive(Clone,Copy)] struct Pointer;
#[derive(Clone)] struct Layer;
#[derive(Clone)] struct PresentedScene {layers:Vec<Layer>,pointer:Pointer}
struct Window {window_id:u64,frame_checked:bool,check_after_ms:u64,check_moved:u32}
#[derive(Debug)] enum CheckError {Moved(String),Failed(String)}
mod io {pub struct PollFd;}
fn monotonic_ms()->Result<u64,()> {Ok(100)}
fn cursor_pointer(_:Pointer)->Option<Pointer>{None}
fn validation_idle(_: &mut [io::PollFd])->bool{true}
fn measure_check<T>(check:impl FnOnce()->T)->T{check()}
const CHECK_MOVED_LIMIT:u32=200;
struct Desktop;
impl Desktop {fn width(&self)->usize{4} fn height(&self)->usize{3}}
struct Compositor {commit:u64,text:String,role:Role,moved:bool,broken:bool,calls:Vec<(u64,bool)>,desktop:Desktop}
impl Compositor {
 fn shown_clients(&self,_:&[Layer],_:Option<Pointer>)->Vec<(u64,[u32;4])>{vec![(8,[0,0,4,3]),(7,[0,0,4,3])]}
 fn sampled_commit(&self,id:u64)->Option<u64>{(id==7||id==8).then_some(self.commit)}
 fn check(&mut self,id:u64,_:&[Layer],_:Option<Pointer>)->Result<u64,CheckError>{self.calls.push((id,false));Ok(12)}
 fn check_area(&mut self,id:u64,_:&[Layer],_:Option<Pointer>,area:[u32;4],want_tree:bool)->Result<(u64,Option<(u64,Vec<u8>)>),CheckError>{
  assert_eq!(area,[0,0,4,3]);self.calls.push((id,want_tree));
  if self.broken{return Err(CheckError::Failed("pixel mismatch".into()))}
  if self.moved{return Err(CheckError::Moved("source changed".into()))}
  let tree=Tree{root:Node{key:"grid".into(),parent:None,role:self.role,name:"Terminal".into(),
   value:serde_json::json!({"cursor":[0,1],"lines":["",self.text]}),bounds:[0,0,4,3],clip:[0,0,4,3],flags:vec![]},children:vec![]};
  let mut page=vec![0;4096];let mut serializations=0;
  tree_commit::serialize_for_commit(READERS.with(|r|r.get()),&mut serializations,||tree.write(&mut page).map(|_|())).unwrap();
  Ok((12,want_tree.then_some((self.commit,page))))
 }
}
mod chrome {
 use super::*;
 @REPORT@
 @DEFERRED@
}
fn present(q:&mut ValidationQueue<PresentedScene>){q.presented(PresentedScene{layers:vec![Layer],pointer:Pointer});}
fn main(){
 // Default production windows have no MAP_TREE reader. CAPTURE pins do not attach one.
 let mut serializations=0;
 let mut page=vec![0;4096];
 tree_commit::serialize_for_commit(0,&mut serializations,||{page[0]=1;Ok(())}).unwrap();
 assert_eq!(serializations,0);assert_eq!(page[0],0);
 let _ = frame_text::cursor_row;
 for mode in ["prompt","empty","moved","broken","other","strict"] {
  println!("BEGIN {mode}");
  let mut q=ValidationQueue::new();present(&mut q);q.first_frame(8);q.first_frame(7);
  let strict=mode=="strict";
  let mut windows=[Window{window_id:7,frame_checked:false,check_after_ms:0,check_moved:1},Window{window_id:8,frame_checked:false,check_after_ms:0,check_moved:0}];
  let mut gpu=Compositor{commit:1,text:if mode=="prompt"{"Starting shell".into()}else{String::new()},role:if mode=="other"{Role::Window}else{Role::Terminal},moved:mode=="moved",broken:mode=="broken",calls:vec![],desktop:Desktop};
  let mut validation=&mut q;
  let mut prompt_watchers=prompt_watch::Watchers::default();
  let strict_startup=strict;
  @REQUEST@
  let _ = (&mut validation,&mut prompt_watchers);
  if !strict {
   assert_eq!(READERS.with(|r|r.get()),2,"default prompt validation must attach semantic readers");
   // The prompt was already rendered before attachment, and there is no new input/output.
   let any_dirty=false;let event_count=0;let font_changed=false;let first_frame=false;
   let tree_reader_attached=READERS.with(|r|r.get())!=0;
   let _ = tree_reader_attached;
   @REDRAW@
   assert!(need_redraw,"an idle terminal must publish its tree after a reader attaches");
  }
  chrome::run_deferred_check(&mut q,&mut gpu,&mut windows,&mut [],false,strict);
  let first=gpu.calls.len();
  // A frame already checked must not cancel its still-empty prompt request.
  q.retain_clients(|id|windows.iter().any(|w|w.window_id==id));
  chrome::run_deferred_check(&mut q,&mut gpu,&mut windows,&mut [],false,strict);
  if mode=="prompt" {
   gpu.commit=2;gpu.text="bsh /> \"\\\n>".into();present(&mut q);
   chrome::run_deferred_check(&mut q,&mut gpu,&mut windows,&mut [],false,strict);
  } else if mode=="empty" || mode=="moved" || mode=="broken" {
   for commit in 2..=CHECK_MOVED_LIMIT as u64+1 {gpu.commit=commit;present(&mut q);chrome::run_deferred_check(&mut q,&mut gpu,&mut windows,&mut [],false,strict);}
  }
  if !strict {assert_eq!(gpu.calls[0],(7,true),"pending prompt must be the subject and request its tree");}
  assert!(first>0);
  let _ = q.pending(7);q.invalidate_scene();
  prompt_watchers.retain(|id|q.prompt_pending(id));
  drop(prompt_watchers);
  assert_eq!(READERS.with(|r|r.get()),0,"all completed or cancelled watchers must release readers");
  println!("END {mode}");
 }
}
'''.replace('@ROOT@', str(ROOT)).replace('@REPORT@', item(config, 'pub(super) fn report_check(')).replace('@DEFERRED@', deferred).replace('@REQUEST@', request).replace('@REDRAW@', redraw)
            if 'pub fn prompt_pending(' not in source('userspace/programs/src/bwm/chrome/validation.rs'):
                program = program.replace('prompt_watchers.retain(|id|q.prompt_pending(id));', 'prompt_watchers.retain(|_|true);')
            # The baseline does not call the compositor's tree API yet.
            program = program.replace('struct Desktop;', '#[allow(dead_code)] struct Desktop;').replace('impl Desktop {', '#[allow(dead_code)] impl Desktop {').replace('struct Compositor {', '#[allow(dead_code)] struct Compositor {').replace('impl Compositor {', '#[allow(dead_code)] impl Compositor {')
            (temp / 'src/main.rs').write_text(program)
            command = ['cargo', 'run', '--quiet', '--manifest-path', str(temp / 'Cargo.toml')]
            if sys.platform == 'darwin':
                command = ['taskpolicy', '-b', *command]
            result = subprocess.run(command, cwd=temp, env={**os.environ, 'CARGO_BUILD_JOBS': '4', 'RUSTFLAGS': '-Dwarnings'}, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            def section(mode):
                return result.stdout.split(f'BEGIN {mode}\n')[1].split(f'END {mode}\n')[0]
            prompt = section('prompt')
            marker = '[bwm] terminal prompt read back: window 7, commit 2, 12 pixels exact, row 1 text "bsh /> \\"\\\\\\n>"\n'
            self.assertNotIn('Starting shell', prompt, 'ordinary startup text must leave the prompt request pending')
            self.assertEqual(prompt.count(marker), 1, prompt)
            self.assertIn('[bwm] composited client frame: window 7 on screen, 12 pixels exact\n' + marker, prompt)
            self.assertNotIn('FAILED', prompt)
            for mode in ['empty', 'moved', 'broken']:
                self.assertEqual(section(mode).count('[bwm] terminal prompt check FAILED: window 7:'), 1, section(mode))
                self.assertNotIn('terminal prompt read back:', section(mode))
            for mode in ['other', 'strict']:
                self.assertNotIn('terminal prompt', section(mode))


if __name__ == '__main__':
    unittest.main()
