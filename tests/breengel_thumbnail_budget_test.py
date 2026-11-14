"""Run the stage's actual sampling commands through the production work validator."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class ThumbnailBudget(unittest.TestCase):
    def test_native_thumbnail_keeps_its_source_and_exact_filter(self):
        source = (ROOT / 'libs/libbreenix/src/suite_stage.rs').read_text()
        methods = '\n'.join(item(source, name) for name in
            ['    fn set_cell(', '    fn sample(', '    fn sample_stage('])
        code = r'''
use std::cell::RefCell;
mod gpu {
 pub use gpu_abi as abi;
 pub fn target_create(_:u32,_:u32,_:u32)->Result<u64,i64>{Ok(3)}
 pub fn resource_destroy(_:u64)->i64{0}
}
const WIDE:(usize,usize)=(400,250);const SQUARE:usize=128;const PENDING:[u8;32]=[255;32];
#[derive(Clone)]struct Cell {hash:[u8;32],pixels:Vec<u8>,width:usize,height:usize}
struct Panes {hash:[u8;32],wide:u64,square:u64,artifacts:Option<()>,sampled:bool,frame:Option<()>}
impl Panes {fn destroy(self){}}
CELL
SAMPLED
struct Gpu {width:u32,height:u32,scanout:[u64;2],shown:usize,atlas:u64,
 cells:Vec<Option<Cell>>,spare:Vec<(u64,u64)>,streams:RefCell<Vec<Vec<u32>>>}
impl Gpu {
 METHODS
 fn keep(&mut self,p:Panes){assert!(p.sampled);assert!(p.artifacts.is_none()&&p.frame.is_none());assert_eq!(p.hash,PENDING);assert!(p.wide>0&&p.square>0);}
 fn run(&self,stream:&[u32],what:&str)->Result<(),String>{
  let resolver=Resources{width:self.width,height:self.height};
  gpu_abi::stream::validate(stream,&resolver).map_err(|e|format!("{what}: code {} at {}",e.error,e.index))?;
  self.streams.borrow_mut().push(stream.to_vec());Ok(())
 }
}
struct Resources{width:u32,height:u32}
impl gpu_abi::stream::Resolver for Resources {
 fn target(&self,h:u64)->Option<gpu_abi::stream::TargetInfo>{Some(gpu_abi::stream::TargetInfo{
  width:if h==2{512}else{400},height:if h==2{384}else{250},format:0,invalid:false})}
 fn source(&self,_:u64)->Option<gpu_abi::stream::SourceInfo>{Some(gpu_abi::stream::SourceInfo{width:self.width,height:self.height,format:0,invalid:false})}
 fn buffer_size(&self,_:u64)->Option<u64>{None}
 fn buffer_rect(&self,_:u64,_:u64)->[i32;4]{unreachable!()}
}
// Inspect the dispatched draws, including their scissor. Each destination pixel
// must use the original whole-frame integer bins exactly once, with no seams.
fn exact_bins(g:&Gpu,tile:usize){
 use gpu_abi::stream::cmd;
 let (cx,cy)=cell(tile);let w=g.width as usize;let h=g.height as usize;
 let (dw,dh)=(w.min(16),h.min(12));let mut visits=vec![0;dw*dh];
 for stream in g.streams.borrow().iter(){
  let mut at=0;let mut target=0;let mut clip=[0,0,512,384];
  while at<stream.len(){let n=stream[at+1] as usize;
   match stream[at]{
    cmd::BEGIN=>{target=stream[at+2];clip=[0,0,512,384];},
    cmd::SCISSOR=>clip.copy_from_slice(&stream[at+2..at+6]),
    _ if target==2=>{
     assert_eq!(stream[at],gpu_abi::draw::COMMAND);
     assert_eq!(stream[at+2],gpu_abi::pipeline::BOX_FILTER);
     // One instance, original destination and source parameters (draw ABI).
     let p=&stream[at+gpu_abi::draw::HEADER+4..at+n];
     assert_eq!(&p[..8],&[cx as u32,cy as u32,dw as u32,dh as u32,0,0,w as u32,h as u32]);
     for y in 0..dh{for x in 0..dw{
      if (cx+x) as u32>=clip[0]&&((cx+x) as u32)<clip[2]&&(cy+y) as u32>=clip[1]&&((cy+y) as u32)<clip[3]{visits[y*dw+x]+=1;}
     }}
    },_=>{}
   }at+=n;
  }
 }
 assert!(visits.iter().all(|&n|n==1),"thumbnail bins must be written exactly once: {visits:?}");
}
#[test]fn native_full_stage_publication_is_bounded_and_retained(){
 for (w,h) in [(2560,2048),(2560,1600),(1280,1024),(7,5)]{
  let mut g=Gpu{width:w,height:h,scanout:[1,1],shown:0,atlas:2,cells:Vec::new(),spare:vec![(3,4)],streams:RefCell::new(Vec::new())};
  let tile=65;let result=g.sample_stage(tile);
  assert!(result.is_ok(),"{w}x{h}: {result:?}; missing cell is the later lost published source");
  let c=g.cells[tile].as_ref().expect("published source must survive until record binds its hash");
  assert_eq!(c.hash,PENDING);assert!(c.pixels.is_empty());assert_eq!((c.width,c.height),(0,0));
  exact_bins(&g,tile);
  if (w,h)==(2560,1600){assert_eq!(g.streams.borrow().len(),2,"legacy frame stays one thumbnail and one pane submission");}
 }
}
'''
        code = code.replace('CELL', item(source, 'fn cell('))
        code = code.replace('SAMPLED', item(source, 'fn sampled(')).replace('METHODS', methods)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'case.rs').write_text(code)
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                '--crate-type', 'rlib', '--cfg', 'feature="alloc"', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(path / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                '--extern', f'gpu_abi={path / "libgpu_abi.rlib"}', str(path / 'case.rs'),
                '-o', str(path / 'case')], check=True)
            subprocess.run([str(path / 'case'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
