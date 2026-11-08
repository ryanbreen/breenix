"""The distinct-window end picture must not depend on boot-global window IDs."""
from pathlib import Path
import re
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class DistinctSeeds(unittest.TestCase):
 def test_production_seeds_keep_the_approved_picture_at_other_id_bases(self):
  source=(ROOT/'userspace/programs/src/suite_breengel/window_cases/windows.rs').read_text()
  body=item(source,'fn windows_distinct_content()')
  painted=re.search(r'w\.frame\((.*?)\)\.cite',body).group(1)
  expected=re.search(r'let want = pattern\(([^,]+), x, y\)',body).group(1)
  pattern=item((ROOT/'userspace/programs/src/suite_breengel/window_cases/kit.rs').read_text(),'pub(crate) fn pattern(')
  run_rust(self,r'''
#[path="@SHA@"]mod digest;
struct W{number:u64}impl W{fn id(&self)->u64{self.number}}
@PATTERN@
fn seed(i:usize,w:&W)->u32{assert!(i<16&&w.id()>0);@PAINTED@}
fn wanted(i:usize,w:&W,x:u32,y:u32)->[u8;4]{assert!(i<16&&w.id()>0);pattern(@EXPECTED@,x,y)}
fn picture(base:u64)->Vec<u8>{let mut out=vec![0;2560*1600*4];for pixel in out.chunks_exact_mut(4){pixel[3]=255;}let mut seeds=Vec::new();for i in 0..16{let w=W{number:base+i as u64};let s=seed(i,&w);for prior in &seeds{assert_ne!(pattern(s,0,0)[..3],pattern(*prior,0,0)[..3],"two windows share content");}seeds.push(s);let ox=100*(i%4)+10;let oy=60*(i/4)+10;for y in 0..50{for x in 0..80{let p=pattern(s,x,y);assert_eq!(p,wanted(i,&w,x,y));let at=((oy+y as usize)*2560+ox+x as usize)*4;out[at..at+3].copy_from_slice(&p[..3]);}}}out}
#[test]fn id_offset_does_not_change_one_pixel(){let first=picture(1);for offset in [130,1025,65537]{assert!(first==picture(offset),"global window-ID offset changed the end frame");}let got=digest::sha256(&first).iter().map(|b|format!("{b:02x}")).collect::<String>();assert_eq!(got,"9fb7ae09341b0f6a24a097d14a872aa9053c329c05ade8df3594a1e8deb2a4bc");}
'''.replace('@SHA@',str(ROOT/'libs/libpixcheck/src/sha256.rs')).replace('@PATTERN@',pattern).replace('@PAINTED@',painted).replace('@EXPECTED@',expected))
if __name__=='__main__':unittest.main()
