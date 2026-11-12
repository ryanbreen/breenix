"""Exercise production UI control bodies after the libuitest ui_app host build."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(path, name):
    text = (ROOT / path).read_text()
    start = text.index('fn ' + name + '(')
    start = text.rfind('\n', 0, start) + 1
    body = text.index('{', start)
    depth, end = 1, body + 1
    while depth:
        depth += (text[end] == '{') - (text[end] == '}')
        end += 1
    return text[start:end]


class Controls(unittest.TestCase):
    def execute(self, source):
        host = subprocess.check_output(['rustc', '-vV'], text=True).split('host: ')[1].splitlines()[0]
        deps = ROOT / 'libs/libuitest/target' / host / 'debug/deps'
        args = []
        for name in ['breengel', 'serde_json']:
            library = max(deps.glob(f'lib{name}-*.rlib'), key=lambda p: p.stat().st_mtime)
            args += ['--extern', f'{name}={library}']
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch)
            (path / 'test.rs').write_text(source)
            result = subprocess.run((['taskpolicy', '-b'] if sys.platform == 'darwin' else []) +
                                    ['rustc', '--edition=2021', '-Dwarnings', '--test', str(path / 'test.rs'),
                                     '-L', f'dependency={deps}', *args, '-o', str(path / 'test')],
                                    capture_output=True, text=True, env=dict(os.environ, CARGO_BUILD_JOBS='4'))
            self.assertEqual(result.returncode, 0, result.stderr)
            result = subprocess.run([str(path / 'test')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_pointer_waits_and_capture_are_reported(self):
        pointer = 'libs/libuitest/src/pointer_live.rs'
        control = (ROOT / 'libs/libbreenix/src/suite_control.rs').read_text().replace(
            '//! Real-time hang watchdogs and fork-shared case diagnostics.', '')
        self.execute(r"""
#![allow(dead_code, unused_imports)]
extern crate self as libbreenix;
pub mod time {
    pub struct Time; impl Time {pub fn as_nanos(&self)->u128{1_000_000}}
    pub fn now_monotonic()->Result<Time,&'static str>{Ok(Time)}
}
mod control {""" + control + r"""}
use control::{Wait,Watchdog};
use std::time::{Instant,Duration};
#[derive(Clone)] struct Event {device:u32,kind:u16,code:u16,value:i32,serial:u64}
struct Delivery {serial:u64,window:u64}
struct Tree; impl Tree {fn parse(_: &[u8])->Result<Self,String>{Ok(Self)}}
mod abi {pub mod limits {pub const MAX_TREE_KIB:u32=1;}}
mod window {
    pub struct Meta {pub processed_serial:u64}
    pub fn read_meta(_:u64)->Result<(u64,Meta),String>{Ok((1,Meta{processed_serial:1}))}
    pub struct Capture {pub commit_id:u64}
    pub fn capture(_:u64,_:&mut [u8],_:Option<&mut [u8]>)->Result<Capture,String>{Ok(Capture{commit_id:1})}
}
mod messages {pub fn input(_: &str,_:u64,_:&str,_:&str)->String{"error".into()}}
mod input {pub mod kind {pub const SYN:u16=0;}}
#[derive(Debug)] enum Errno {EAGAIN}
#[derive(Debug)] enum Error {Os(Errno), Other}
impl std::fmt::Display for Error {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"error")}}
struct Record {event:Event}
struct Reader; impl Reader {fn read(&mut self,_:&mut [u8])->Result<Vec<Result<Record,Error>>,Error>{Err(Error::Os(Errno::EAGAIN))}}
struct Session; impl Session {fn input_devices(&self)->[u32;1]{[1]}}
struct Info {width:u32,height:u32}
struct Live {pending:Vec<Event>,reader:Reader,session:Session,serials:Vec<u64>}
fn outcome(_:u64)->Result<Option<(String,u64,String)>,String>{Ok(Some(("delivered".into(),1,String::new())))}
fn pause()->Result<(),String>{Ok(())}
fn first_missing_disposition(_: &str,_:&[u64])->Result<Option<u64>,String>{Ok(None)}
fn client_processed(_:u64,_:u64)->bool{true}
""" + function(pointer, 'wait_delivery').replace('super::client_processed', 'client_processed').replace('crate::messages', 'messages') +
            function(pointer, 'wait_pointer_idle').replace('crate::messages', 'messages') +
            function(pointer, 'dispositions').replace('super::first_missing_disposition', 'first_missing_disposition').replace('crate::messages', 'messages') + r"""
impl Live {
fn info(&self,_:u64)->Result<Info,String>{Ok(Info{width:1,height:1})}
""" + function(pointer, 'capture') + function(pointer, 'source_event') + r"""
}
#[test] fn report_each_body() {
    let scope=control::Scope::new().unwrap();
    let mut live=Live{pending:vec![Event{device:1,kind:1,code:1,value:1,serial:1}],reader:Reader,session:Session,serials:vec![]};
    let delivery=wait_delivery(1,None,false).unwrap(); assert_eq!((delivery.serial,delivery.window),(1,1));
    wait_pointer_idle(1).unwrap();
    let _=dispositions(&[1]);
    assert_eq!(live.source_event(1,1,1,1).unwrap().serial,1);
    live.capture(1).unwrap();
    let rows=scope.report("suite","case");
    assert!(rows.iter().any(|r|r.contains("wait=InputDelivery started=4 completed=4")),"{rows:?}");
    assert!(rows.iter().any(|r|r.contains("wait=TreeCapture started=1 completed=1")),"{rows:?}");
}
""")

    def test_modifier_release_after_last_window_closes(self):
        self.execute('''
pub struct Event {pub serial:u64}
pub struct Live;
impl Live {
pub fn send_key(&mut self,_code:u16,_pressed:bool)->Result<Event,String>{Ok(Event{serial:1})}
''' + function('libs/libuitest/src/pointer_live.rs', 'wm_key') + '''
}
pub fn wait_pointer_idle(_serial:u64)->Result<(),String>{Err("rejected no-focus".into())}
pub fn wait_key_release(_serial:u64)->Result<(),String>{Ok(())}
#[test] fn release_without_focus(){assert!(Live.wm_key(56,false).is_ok());assert!(Live.wm_key(56,true).is_err());}
''')

    def test_ready_resize_receipts_do_not_sleep(self):
        self.execute('''
use std::sync::atomic::{AtomicU64,Ordering};
static SLEPT:AtomicU64=AtomicU64::new(0);
mod time {pub fn sleep_ms(n:u64)->Result<(),String>{super::SLEPT.fetch_add(n,super::Ordering::Relaxed);Ok(())}}
mod key {pub const BUTTON_LEFT:u16=272;}
pub mod input {pub fn wait_pointer_idle(_serial:u64)->Result<(),String>{Ok(())}}
pub struct Event {pub serial:u64}
pub struct Info {x:i32,y:i32,width:u32,height:u32}
pub struct Live {w:u32,h:u32}
impl Live {
fn info(&self,_id:u64)->Result<Info,String>{Ok(Info{x:0,y:0,width:self.w,height:self.h})}
fn move_to(&mut self,_point:(i32,i32))->Result<(),String>{Ok(())}
pub fn session(&mut self)->&mut Self {self}
pub fn raw_press(&mut self,_code:u16)->Result<(),String>{Ok(())}
pub fn raw_release(&mut self,_code:u16)->Result<(),String>{Ok(())}
pub fn raw_motion(&mut self,x:i32,y:i32)->Result<(),String>{self.w=(self.w as i32+x) as u32;self.h=(self.h as i32+y) as u32;Ok(())}
pub fn send_button(&mut self,_code:u16,_down:bool)->Result<Event,String>{Ok(Event{serial:1})}
pub fn wm_motion(&mut self,delta:(i32,i32))->Result<(),String>{self.raw_motion(delta.0,delta.1)}
}
pub struct Keys(Live);impl Keys{fn backend_mut(&mut self)->&mut Live{&mut self.0}}
pub struct App {keys:Keys,window:u64}
enum Wait {AppOutput}
struct Watchdog;impl Watchdog{fn new(_wait:Wait)->Result<Self,String>{Ok(Self)} fn deadline_ns(&self)->u64{u64::MAX}}
fn now()->Result<u64,String>{Ok(0)}
impl App {
''' + function('libs/libuitest/src/app.rs', 'resize') + '''
}
#[test] fn six_ready_apps(){for i in 0..6 {let mut t=App{keys:Keys(Live{w:if i==5{600}else{400},h:if i==5{480}else{300}}),window:1};t.resize(300,200).unwrap();assert_eq!((t.keys.0.w,t.keys.0.h),(300,200));}assert_eq!(SLEPT.load(Ordering::Relaxed),0,"ready receipts must not add 5400 ms of blind delay");}
''')

    def test_peer_checks_share_one_tree_snapshot(self):
        self.execute('''
use serde_json::{json,Value};
use breengel::tree::{Tree,Node,Role};
struct Expected;impl Expected{fn digest(&self)->String{"expected".into()}}
pub struct App{tree:Tree,reads:usize}
impl App{
pub fn wait_node(&mut self,key:&str,p:impl Fn(&Value)->bool)->Result<(),String>{self.reads+=1;let n=self.tree.children.iter().find(|n|n.key==key).unwrap();if p(&n.value){Ok(())}else{Err("mismatch".into())}}
pub fn wait_tree(&mut self,p:impl Fn(&Tree)->bool)->Result<Tree,String>{self.reads+=1;if p(&self.tree){Ok(self.tree.clone())}else{Err("mismatch".into())}}
}
fn attach(_t:&mut App,_owner:&str)->Result<(),String>{Ok(())}
''' + function('userspace/programs/src/guiapps_suite/guskit_collab.rs', 'check_peer') + '''
fn node(key:&str,value:Value)->Node{Node{key:key.into(),parent:Default::default(),role:Role::Status,name:String::new(),value,bounds:[0;4],clip:[0;4],flags:vec![]}}
#[test]fn coherent_peer_query(){for (applied,digest,ok) in [(json!([1]),"expected",true),(json!([]),"expected",false),(json!([1]),"stale",false)]{let mut t=App{tree:Tree{root:node("root",Value::Null),children:vec![node("collab",json!({"applied":applied})),node("canvas",json!({"width":200,"height":120,"sha256":digest}))]},reads:0};assert_eq!(check_peer(&mut t,"Host",&Expected,&json!([1])).is_ok(),ok);if ok{assert_eq!(t.reads,1,"one coherent tree must answer both assertions");}}}
''')


    def test_answer_replay_does_not_recapture_the_same_tree(self):
        self.execute('''use serde_json::{json,Value};
use breengel::tree::{Tree,Node,Role};
pub enum Op {Stroke{y:usize,color:[u8;3]},Undo}
pub struct Submitted {owner:&'static str,id:u64,op:Op,refusal:Option<&'static str>}
struct Script(Vec<Submitted>,std::collections::BTreeMap<String,Value>);
struct Expected;impl Expected{fn white()->Self{Self} fn colored_horizontal(&mut self,_a:usize,_b:usize,_y:usize,_c:[u8;3]){}}
pub struct App{tree:Tree,reads:usize}
impl App{
pub fn wait_node(&mut self,_key:&str,p:impl Fn(&Value)->bool)->Result<(),String>{self.reads+=1;if p(&self.tree.children[0].value){Ok(())}else{Err("mismatch".into())}}
pub fn wait_tree(&mut self,p:impl Fn(&Tree)->bool)->Result<Tree,String>{self.reads+=1;if p(&self.tree){Ok(self.tree.clone())}else{Err("mismatch".into())}}
pub fn tree(&mut self)->Result<Tree,String>{self.reads+=1;Ok(self.tree.clone())}
}
fn attach(_t:&mut App,_owner:&str)->Result<(),String>{Ok(())}
pub fn collab_in(tree:&Tree)->Result<Value,String>{Ok(tree.children[0].value.clone())}
pub fn collab(t:&mut App)->Result<Value,String>{collab_in(&t.tree()?)}
impl Script{
''' + function('userspace/programs/src/guiapps_suite/guskit_collab.rs', 'replay') + '''}
#[test]fn one_read_per_owner(){let node=Node{key:"collab".into(),parent:Default::default(),role:Role::Status,name:String::new(),value:json!({"submitted":[],"applied":[]}),bounds:[0;4],clip:[0;4],flags:vec![]};let mut root=node.clone();root.key="root".into();let mut t=App{tree:Tree{root,children:vec![node]},reads:0};let mut script=Script(vec![],Default::default());script.replay(&mut t,&["Host","Peer"]).unwrap();assert_eq!(t.reads,2,"answered rows must come from the tree already read by the wait");}
''')


if __name__ == '__main__':
    unittest.main()
