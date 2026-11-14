"""Stage geometry admits the native monitor's extra rows at each display scale."""
import pathlib
import subprocess
import tempfile
import unittest
import sys
ROOT=pathlib.Path(__file__).resolve().parents[1]
REV=sys.argv.pop() if len(sys.argv)>1 and not sys.argv[-1].startswith('-') else None
def source(path):
    return subprocess.check_output(['git','show',f'{REV}:{path}'],cwd=ROOT,text=True) if REV else (ROOT/path).read_text()
def item(s,needle):
    a=s.index(needle);b=s.index('{',a)+1;pos=b;depth=1
    while depth:
        depth+=(s[pos]=='{')-(s[pos]=='}');pos+=1
    return s[a:pos]
class StageGeometry(unittest.TestCase):
    def test_native_and_virtual_stage_extents(self):
        code='''#[derive(Debug,PartialEq)] enum StageError {Size}
struct Surface{w:usize,h:usize} impl Surface{fn width(&self)->usize{self.w} fn height(&self)->usize{self.h}}
struct FrameBuf{width:usize,height:usize}
'''
        for label,path in [('gpu','libs/libgfx/src/stage.rs'),('reference','libs/libgfx-ref/src/stage.rs')]:
            code+=item(source(path),'fn scale_of(').replace('fn scale_of(',f'fn {label}(')+'\n'
        code+='''fn main(){for (w,h,n) in [(1280,1024,1),(2560,2048,2),(1280,800,1),(2560,1600,2)] {
 assert_eq!(gpu(&Surface{w,h}),Ok(n));assert_eq!(reference(&FrameBuf{width:w,height:h}),Ok(n));
}
for (w,h) in [(0,0),(1024,768),(2560,1599),(2561,2048)] {
 assert_eq!(gpu(&Surface{w,h}),Err(StageError::Size));assert_eq!(reference(&FrameBuf{width:w,height:h}),Err(StageError::Size));
}}
'''
        with tempfile.TemporaryDirectory() as tmp:
            tmp=pathlib.Path(tmp);(tmp/'check.rs').write_text(code)
            subprocess.run(['rustc','--edition=2021',str(tmp/'check.rs'),'-o',str(tmp/'check')],check=True)
            subprocess.run([str(tmp/'check')],check=True)
if __name__=='__main__':unittest.main()
