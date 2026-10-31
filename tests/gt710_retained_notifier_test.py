"""Run the native retained-binding publication with a device-written notifier."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class RetainedNotifier(unittest.TestCase):
    def test_fresh_disjoint_report_and_consumed_ring_wrap(self):
        native = (ROOT / 'kernel/src/gpu/gk208/native.rs').read_text()
        code = item(native, '\nfn acknowledge(') + '\n' + item(native, '\nfn submit(')
        fixture = r'''
use std::collections::BTreeMap;
struct Registers { memory: BTreeMap<u64,u32>, writes: Vec<(u64,Vec<u32>)>,
    get:u32, notify:u32, write_done:bool }
struct Evo { notifier:u64,total:u64,push:u64,put:u32,limits:(),coefficients:(),
    head:u32,sor:u32,scripts:[Vec<()>;4] }
fn write_words(r:&mut Registers,_:u64,start:u64,words:&[u32])->Result<(),&'static str>{
    r.writes.push((start,words.to_vec()));
    for (n,&v) in words.iter().enumerate(){r.memory.insert(start+n as u64*4,v);}Ok(())
}
fn read_word(r:&mut Registers,_:u64,a:u64)->Result<u32,&'static str>{Ok(*r.memory.get(&a).unwrap_or(&0))}
fn complete(r:&mut Registers,total:u64,report:u64,_:&(),_:&(),_:(u32,u32,bool,bool),_:&[Vec<()>;4])
    ->Result<u32,&'static str>{let v=read_word(r,total,report)?;if v&1!=0{Ok(v)}else{Err("no fresh DONE")}}
mod modeset {
    pub fn wait(r:&mut super::Registers,register:usize,mask:u32,value:u32)->Result<(),&'static str>{
        assert_eq!((register,mask),(0x640004,u32::MAX));assert_eq!(r.get,value);Ok(())
    }
    pub fn publish(r:&mut super::Registers,put:u32)->Result<(),&'static str>{
        if put==0{r.get=0;return Ok(());}
        let (start,words)=r.writes.last().unwrap().clone();assert_eq!(start,0x2000+u64::from(r.get));
        for pair in words.chunks_exact(2){match pair[0]{
            0x40084=>r.notify=pair[1],
            0x4008c=>{assert_eq!(r.notify,0x80000200);assert_eq!(pair[1],0);
                assert_eq!(r.memory[&0x1210],0,"old DONE must be cleared before the device request");
                if r.write_done{r.memory.insert(0x1210,0xabcd0001);}},
            _=>panic!("unexpected head change {:?}",pair),
        }}r.get=put;Ok(())
    }
}
CODE
fn setup(put:u32,write_done:bool)->(Evo,Registers){
    (Evo{notifier:0x1000,total:1<<30,push:0x2000,put,limits:(),coefficients:(),head:0,sor:0,
        scripts:std::array::from_fn(|_|Vec::new())},
     Registers{memory:BTreeMap::from([(0x1000,0x13570001),(0x1010,0x24680001),(0x1210,0xdead0001)]),
        writes:vec![],get:put,notify:0,write_done})
}
#[test]fn fresh_capability_done_uses_a_disjoint_report(){
    let(mut e,mut r)=setup(0,true);assert_eq!(acknowledge(&mut e,&mut r).unwrap(),0xabcd0001);
    assert_eq!((r.memory[&0x1000],r.memory[&0x1010]),(0x13570001,0x24680001));
    assert_eq!((e.put,r.get,r.notify),(24,24,0));
}
#[test]fn stale_done_cannot_complete_a_withheld_request(){
    let(mut e,mut r)=setup(0,false);assert!(acknowledge(&mut e,&mut r).is_err());
    assert_eq!(r.memory[&0x1210],0);
}
#[test]fn consumed_ring_wrap_precedes_new_commands(){
    let(mut e,mut r)=setup(2032,true);assert_eq!(acknowledge(&mut e,&mut r).unwrap(),0xabcd0001);
    assert!(r.writes.contains(&(0x2000+2032,vec![0x20000000])));assert_eq!(e.put,24);
}
'''.replace('CODE', code)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            (p / 'notifier.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(p / 'notifier.rs'), '-o', str(p / 'notifier')], check=True)
            subprocess.run([str(p / 'notifier'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
