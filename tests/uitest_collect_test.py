"""Run the local collector with real files/codecs and controlled device observations."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item
ROOT = Path(__file__).resolve().parents[1]
class Producers(unittest.TestCase):
    def test_screen_window_remnant_arena_and_stderr_are_saved(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            def run(args):
                if sys.platform == 'darwin': args = ['taskpolicy', '-b', *args]
                result = subprocess.run(args, cwd=ROOT, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                return result
            for name, source, deps in [
                ('libpixcheck', 'libs/libpixcheck/src/lib.rs', []),
                ('breengel_abi', 'libs/breengel-abi/src/lib.rs', []),
                ('libimg', 'libs/libimg/src/lib.rs', []),
                ('libimg_encode', 'libs/libimg-encode/src/lib.rs', ['libimg']),
            ]:
                run(['rustc', '--edition=2021', '-Dwarnings', '-O', '--crate-name', name, '--crate-type=rlib', source,
                    '-L', str(path), *sum((['--extern', f'{dep}={path}/lib{dep}.rlib'] for dep in deps), []), '-o', str(path/f'lib{name}.rlib')])
            source = (ROOT/'libs/libuitest/src/collect.rs').read_text()
            source = source.replace('"/var/uitest"', 'env!("COLLECT_ROOT")')
            source = source.replace('"/proc/breenix/input-ledger"', 'env!("COLLECT_INPUT")').replace('"/proc/breenix/composites"', 'env!("COLLECT_COMPOSITES")')
            (path/'collect.rs').write_text(source)
            code = r'''
extern crate self as libbreenix;
use std::{collections::BTreeSet,sync::atomic::{AtomicBool,Ordering},path::Path};
#[path="ARENA"]pub mod arena;
#[path="ARTIFACTS"]pub mod artifacts;
#[path="RESULT"]pub mod result;
#[path="COLLECTION"]pub mod collection;
extern crate self as breenix_suite;
pub mod suite{pub use crate::collection;}
#[path="COLLECT"]mod collect;
pub mod errno{#[derive(Debug)]pub enum Errno{ENOENT,EIO}}
pub mod error{
    #[derive(Debug)]pub enum Error{Os(super::errno::Errno)}
    impl std::fmt::Display for Error{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"{:?}",self)}}
}
pub mod gpu{
    pub struct Mode{pub width:u32,pub height:u32}
    pub fn current_mode()->Result<Mode,i64>{Ok(Mode{width:8,height:4})}
}
static LIVE:AtomicBool=AtomicBool::new(true);
pub mod window{
    pub use breengel_abi as abi;
    #[derive(PartialEq)]pub enum ObservationKind{Plain,Composed}
    VALIDATOR
    pub fn list(rows:&mut[abi::WinInfo])->Result<usize,super::error::Error>{
        if !super::LIVE.load(super::Ordering::SeqCst){return Ok(0)}
        rows[0]=abi::WinInfo{id:17,owner:1234,width:8,height:4,commit_id:7,..abi::WinInfo::default()};Ok(1)
    }
    pub fn call<T:abi::Plain>(op:u32,arg:&mut T)->Result<u64,super::error::Error>{
        unsafe{match op{
            abi::op::HOLD_EVIDENCE|abi::op::RELEASE_REMNANT=>{},
            abi::op::SCANOUT_READBACK=>{
                assert_eq!(std::mem::size_of::<T>(),std::mem::size_of::<abi::Readback>());
                let rb=&mut*(arg as *mut T as *mut abi::Readback);
                let pixels=std::slice::from_raw_parts_mut(rb.dst as *mut u8,rb.dst_len as usize);pixels.fill(255);
                let record=std::slice::from_raw_parts_mut(rb.record as *mut u8,1024);
                for(at,n)in [(0,2u32),(4,1024),(16,4),(20,4),(24,1),(40,8),(44,4),(56,32),(128,0x1100),(132,0x206),(192,0x1100)]{record[at..at+4].copy_from_slice(&n.to_le_bytes());}
                for at in [80,120,184]{record[at..at+8].copy_from_slice(&1u64.to_le_bytes());}
            },
            abi::op::CAPTURE|abi::op::READ_REMNANT=>{
                assert_eq!(std::mem::size_of::<T>(),std::mem::size_of::<abi::Capture>());
                let c=&mut*(arg as *mut T as *mut abi::Capture);c.width=8;c.height=4;c.commit_id=7;
                let p=std::slice::from_raw_parts_mut(c.dst as *mut u8,128);
                for pixel in p.chunks_exact_mut(4){pixel.copy_from_slice(&[21,31,41,255]);}
                std::slice::from_raw_parts_mut(c.tree_dst as *mut u8,4).copy_from_slice(b"tree");
            },
            _=>panic!("unexpected op {op}"),
        }}Ok(0)
    }
}
#[test]fn local_sources_and_remnants(){
    let root=Path::new(env!("COLLECT_ROOT"));std::fs::create_dir_all(root).unwrap();
    std::fs::write(env!("COLLECT_INPUT"),b"serial=9 delivered\n").unwrap();
    std::fs::write(env!("COLLECT_COMPOSITES"),b"1 100 17 7 1\n").unwrap();
    // Check-owned files must survive the collector opening this same case path.
    let mut saved=artifacts::Store::new(root,"test","runner/live").unwrap();
    saved.write("pixels.got.png",b"check-owned").unwrap();
    let config=root.join("config");std::fs::create_dir(&config).unwrap();
    let mut evidence=collect::Evidence::new().unwrap();evidence.configure(&config).unwrap();
    let mut case=arena::Arena::open(&config.join("evidence.arena")).unwrap();
    let mut frame=case.begin(arena::Slot::Frame).unwrap();frame.bytes_mut()[0]=3;drop(frame);
    evidence.observe(999,&BTreeSet::from([1234])).unwrap();assert!(evidence.holds(17));
    let reply=evidence.collect("test","runner/live",collection::Trigger::Crash,&[(1234,4,b"stderr".to_vec())]);
    assert_eq!(reply,"SUITE test COLLECT runner/live trigger=crash screen=at-collection windows=1 remnants=0 artifacts=complete\n");
    let out=root.join("test/runner/live");
    assert_eq!(std::fs::read(out.join("pixels.got.png")).unwrap(),b"check-owned");
    assert_eq!(libimg::png::decode(&std::fs::read(out.join("screen.png")).unwrap()).unwrap().pixels,vec![255;128]);
    let image=libimg::png::decode(&std::fs::read(out.join("window-17-7.png")).unwrap()).unwrap();
    assert!(image.pixels.chunks_exact(4).all(|p|p==[21,31,41,255]));
    assert_eq!(&std::fs::read(out.join("window-17-7.tree")).unwrap()[..4],b"tree");
    assert_eq!(std::fs::read(out.join("stderr-1234.log")).unwrap(),b"stderr");
    assert_eq!(std::fs::read(out.join("input.log")).unwrap(),b"serial=9 delivered\n");
    let manifest=std::fs::read_to_string(out.join("collection.txt")).unwrap();
    assert!(manifest.contains("frame=partial:case-died")&&manifest.contains("stderr-1234-dropped=4"));
    let mut slot=case.begin(arena::Slot::Screen).unwrap();slot.bytes_mut()[..128].fill(19);
    slot.publish(arena::Record{check:"failure-screen".into(),width:8,height:4,
        observation:std::fs::read(out.join("screen.record")).unwrap(),..arena::Record::default()},128).unwrap();
    LIVE.store(false,Ordering::SeqCst);
    let reply=evidence.collect("test","runner/dead",collection::Trigger::Timeout,&[]);
    assert!(reply.contains("windows=0 remnants=1 artifacts=complete"),"{reply}");
    assert!(root.join("test/runner/dead/remnant-17-7.png").is_file());
    let dead=root.join("test/runner/dead");
    assert!(reply.contains("screen=at-failure"));
    let failure=libimg::png::decode(&std::fs::read(dead.join("screen.png")).unwrap()).unwrap();
    assert!(failure.pixels.chunks_exact(4).all(|p|p==[19,19,19,255]));
    assert_eq!(libimg::png::decode(&std::fs::read(dead.join("screen-at-collection.png")).unwrap()).unwrap().pixels,vec![255;128]);
    evidence.release_remnants();
    let config=root.join("oversize-config");std::fs::create_dir(&config).unwrap();
    let mut oversize=collect::Evidence::new().unwrap();oversize.configure(&config).unwrap();
    let mut case=arena::Arena::open(&config.join("evidence.arena")).unwrap();
    let mut slot=case.begin(arena::Slot::Screen).unwrap();slot.bytes_mut().fill(27);
    slot.publish(arena::Record{width:16,height:4,..arena::Record::default()},256).unwrap();
    oversize.collect("test","runner/oversize",collection::Trigger::Crash,&[]);
    let out=root.join("test/runner/oversize");
    assert!(std::fs::read_to_string(out.join("collection.txt")).unwrap().contains("screen=partial:oversize"));
    assert_eq!(std::fs::read(out.join("screen-at-failure.partial.bgrx")).unwrap(),vec![27;128]);
}
'''
            code = code.replace('ARENA', str(ROOT/'libs/libuitest/src/arena.rs')).replace('ARTIFACTS', str(ROOT/'libs/libuitest/src/artifacts.rs')).replace('RESULT', str(ROOT/'libs/libbreenix/src/suite_result.rs')).replace('COLLECTION', str(ROOT/'libs/libbreenix/src/suite_collection.rs')).replace('\"COLLECT\"', '\"'+str(path/'collect.rs')+'\"')
            validator = item((ROOT/'libs/libbreenix/src/window.rs').read_text(), 'pub fn validate_scanout_observation(')
            code = code.replace('VALIDATOR', validator)
            (path/'check.rs').write_text(code)
            import os
            env = os.environ.copy()
            env.update(COLLECT_ROOT=str(path/'out'),COLLECT_INPUT=str(path/'input'),COLLECT_COMPOSITES=str(path/'composites'))
            args=['rustc','--edition=2021','--test','-Dwarnings','-O',str(path/'check.rs'),'-L',str(path)]
            for dep in ['libpixcheck','breengel_abi','libimg','libimg_encode']:args+=['--extern',f'{dep}={path}/lib{dep}.rlib']
            args+=['-o',str(path/'check')]
            if sys.platform == 'darwin':args=['taskpolicy','-b',*args]
            result=subprocess.run(args,cwd=ROOT,env=env,capture_output=True,text=True)
            self.assertEqual(result.returncode,0,result.stdout+result.stderr)
            result=run([str(path/'check')]);print(result.stdout)
if __name__ == '__main__': unittest.main()
