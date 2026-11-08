"""Text runs at one size share cold raster submission and preserve placement."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class TextRuns(unittest.TestCase):
 def test_cold_runs_keep_offsets_sizes_errors_and_one_submission(self):
  source=(ROOT/'libs/libgfx/src/ttf_font.rs').read_text()
  legacy='''fn rasterize_text_runs(font: &mut CachedFont, runs: &[(Vec<(u16,i64)>, i32)], lcd: bool, mut out: impl FnMut(usize,Result<(Arc<AtlasGlyph>,i64),Error>)) { for (i,(placed,size)) in runs.iter().enumerate(){font.run_mode(placed,*size,lcd,|got|out(i,got));} }'''
  body=item(source,'fn rasterize_text_runs(') if 'fn rasterize_text_runs(' in source else legacy
  run_rust(self,r'''
use std::sync::Arc;
type AtlasGlyph=u16;#[derive(Debug)]enum Error{Device(i64)}
#[derive(Default)]struct CachedFont{calls:Vec<(i32,bool,usize)>}
impl CachedFont{fn run_mode(&mut self,placed:&[(u16,i64)],size:i32,lcd:bool,mut out:impl FnMut(Result<(Arc<AtlasGlyph>,i64),Error>)){self.calls.push((size,lcd,placed.len()));for &(glyph,dx) in placed {if glyph==99{out(Err(Error::Device(-5)))}else{out(Ok((Arc::new(glyph),dx)))}}}}
@BODY@
#[test]fn cold_text_batch(){
 let runs=vec![(vec![(1,-7),(2,15)],896),(vec![(3,400),(4,411)],896)];let mut font=CachedFont::default();let mut got=vec![];
 rasterize_text_runs(&mut font,&runs,true,|run,value|{let(g,dx)=value.unwrap();got.push((run,*g,dx));});
 assert_eq!(got,vec![(0,1,-7),(0,2,15),(1,3,400),(1,4,411)]);
 assert_eq!(font.calls,vec![(896,true,4)],"two taskbar runs need one cold raster run/wait");
 let runs=vec![(vec![(5,1)],640),(vec![(6,2)],896),(vec![(7,3)],640)];let mut got=vec![];font.calls.clear();
 rasterize_text_runs(&mut font,&runs,false,|run,value|{let(g,dx)=value.unwrap();got.push((run,*g,dx));});
 assert_eq!(font.calls,vec![(640,false,1),(896,false,1),(640,false,1)]);assert_eq!(got,vec![(0,5,1),(1,6,2),(2,7,3)]);
 let mut errors=0;rasterize_text_runs(&mut font,&[(vec![(99,0)],896)],true,|_,v|match v{Err(Error::Device(e))=>{assert_eq!(e,-5);errors+=1},_=>panic!()});assert_eq!(errors,1);
}
'''.replace('@BODY@',body))
if __name__=='__main__':unittest.main()
