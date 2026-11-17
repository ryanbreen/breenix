"""present/flip leaves the stage's display mode in place for its end checkpoint.

The case's authored end screen (docs/suites/gpu.json, present/flip expectEnd) is
the flipped source composed into the display's own mode. The `end` checkpoint
observes the mode current when the case returns (docs/gui/screen-proof.md), so
a SET_MODE inside the case changes that picture. This runs the real case body
against the real MODES type with a modelled display and checks the source size
and the mode left at case end.
"""
from pathlib import Path
import json
import re
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item

HARNESS=r'''
use gpu_abi as abi;
use std::cell::RefCell;
type CaseError=String;
type CaseResult=Result<(),CaseError>;
type R<T>=Result<T,CaseError>;
const A:u32=0xffd15321;
const B:u32=0xff25b3e7;
mod format {pub const B8G8R8A8:u32=1;}
struct Canvas{w:u32,h:u32}
struct Display{modes:abi::Modes,targets:Vec<(u32,u32)>,shown:Vec<(u32,u32,u32,u32)>,fence:u64}
thread_local!{static D:RefCell<Option<Display>>=RefCell::new(None);}
fn with<T>(f:impl FnOnce(&mut Display)->T)->T{D.with(|d|f(d.borrow_mut().as_mut().unwrap()))}
fn begin()->R<()>{Ok(())}
fn fail_msg(s:String)->CaseError{s}
fn ensure(test:bool,message:impl FnOnce()->String)->CaseResult{if test{Ok(())}else{Err(message())}}
fn catalogue()->R<abi::Modes>{Ok(with(|d|d.modes))}
#[allow(dead_code)]
fn mode(index:u32)->R<()>{with(|d|{ensure(index<d.modes.count,||"SET_MODE index".into())?;d.modes.current=index;Ok(())})}
fn scanout_target(_format:u32,w:u32,h:u32)->R<u64>{Ok(with(|d|{d.targets.push((w,h));d.targets.len() as u64}))}
fn scene(_t:u64,_colour:u32,_rects:&[([i32;4],u32)]){}
fn scene_canvas(w:u32,h:u32,_colour:u32,_rects:&[([i32;4],u32)])->Canvas{Canvas{w,h}}
fn frame(_c:u64,_s:&())->R<()>{Ok(())}
fn setup(w:u32,h:u32,colour:u32)->R<(u64,u64,Canvas)>{let t=scanout_target(format::B8G8R8A8,w,h)?;Ok((1,t,scene_canvas(w,h,colour,&[])))}
fn present(_c:u64,_t:u64)->R<u64>{Ok(with(|d|{d.fence+=1;d.fence}))}
fn shown(t:u64,want:&Canvas,_fence:u64)->R<u64>{with(|d|{
    let (tw,th)=d.targets[t as usize-1];
    ensure((tw,th)==(want.w,want.h),||"presented source dimensions changed".into())?;
    let m=d.modes.current_mode().unwrap();
    d.shown.push((tw,th,m.width,m.height));Ok(t)})}
BODY
fn catalogue_of(list:&[(u32,u32,u32)],current:u32)->abi::Modes{
    let zero=abi::Mode{width:0,height:0,scale:0,reserved:0};
    let mut modes=[zero;2];let mut extra=[zero;8];
    for(i,&(width,height,scale))in list.iter().enumerate(){
        let m=abi::Mode{width,height,scale,reserved:0};
        if i<2{modes[i]=m}else{extra[i-2]=m}}
    abi::Modes{version:1,size:256,count:list.len() as u32,current,max_width:16384,max_height:16384,
        owner_depth:1,binding_known:1,modes,scanout_resource:1,console_resource:1,console_draws:0,
        console_fixture_bytes:0,handoff:0,firmware_hash_at_handoff:0,firmware_hash_now:0,
        firmware_writes_after_handoff:0,extra_modes:extra}}
fn run(list:&[(u32,u32,u32)],stage:u32)->((u32,u32),Vec<(u32,u32,u32,u32)>){
    D.with(|d|*d.borrow_mut()=Some(Display{modes:catalogue_of(list,stage),targets:vec![],shown:vec![],fence:0}));
    flip().unwrap();
    with(|d|{let m=d.modes.current_mode().unwrap();((m.width,m.height),d.shown.clone())})}
fn main(){
    // Virtio: the stage runs at the scale-2 mode, as every ARM capture records.
    let (end,shown)=run(&[(1280,800,1),(2560,1600,2)],1);
    assert_eq!(shown.len(),3,"flip presents A, B, A");
    for &(sw,sh,_,_) in &shown {assert_eq!((sw,sh),(SRC_W,SRC_H),"flip source differs from the authored source");}
    assert_eq!(end,(END_W,END_H),"flip left the display at {}x{}, not the authored end mode",end.0,end.1);
    // GT710 monitor fixture: the stage runs at the preferred native mode.
    let (end,shown)=run(&[(1024,768,1),(1280,1024,1),(2560,2048,2)],1);
    for &(sw,sh,dw,dh) in &shown {assert_eq!((sw,sh,dw,dh),(1280,1024,1280,1024),"native flip no longer fills its native mode");}
    assert_eq!(end,(1280,1024),"flip changed the native stage mode");
}
'''

class FlipEndModeTest(unittest.TestCase):
    def test_flip_keeps_the_stage_mode_for_its_end_screen(self):
        suite=json.loads((ROOT/'docs/suites/gpu.json').read_text())
        case=next(c for cat in suite['categories'] if cat['id']=='present'
            for c in cat['cases'] if c['id']=='flip')
        src=re.search(r'\bA (\d+)x(\d+) orange-red',case['expect'])
        end=re.search(r'extends across (\d+)x(\d+)',case['expectEnd'])
        self.assertTrue(src and end,'present/flip expectations no longer name source and end sizes')
        body=item((ROOT/'userspace/programs/src/suite_gpu/present.rs').read_text(),'pub fn flip(')
        code=(HARNESS.replace('BODY',body)
            .replace('SRC_W',src[1]).replace('SRC_H',src[2])
            .replace('END_W',end[1]).replace('END_H',end[2]))
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'check.rs').write_text(code)
            subprocess.run(priority+['rustc','--edition=2021','--crate-type=rlib','--crate-name=gpu_abi',
                '-Awarnings',str(ROOT/'libs/gpu-abi/src/lib.rs'),'--out-dir',str(p)],check=True)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings','-L',str(p),
                '--extern',f'gpu_abi={p/"libgpu_abi.rlib"}',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)

if __name__=='__main__':unittest.main()
