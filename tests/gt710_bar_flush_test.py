"""Short native BAR drains should not incur a 100-us polling floor."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class BarFlush(unittest.TestCase):
    def test_short_drain_and_unchanged_stuck_timeout(self):
        source = (ROOT / 'kernel/src/gpu/gk208/modeset.rs').read_text()
        code = r'''
pub trait Io {fn read(&mut self,r:usize)->u32;fn write(&mut self,r:usize,v:u32);fn delay_us(&mut self,us:u32);}
WAIT
FLUSH
struct Model {busy:usize,reads:usize,delayed:u64,writes:Vec<(usize,u32)>}
impl Model {fn new(busy:usize)->Self{Self{busy,reads:0,delayed:0,writes:Vec::new()}}}
impl Io for Model {
 fn read(&mut self,r:usize)->u32{assert_eq!(r,0x70000);self.reads+=1;if self.reads<=self.busy{2}else{1}}
 fn write(&mut self,r:usize,v:u32){self.writes.push((r,v));}
 fn delay_us(&mut self,us:u32){self.delayed+=u64::from(us);}
}
fn main(){
 for busy in [0,1] {let mut m=Model::new(busy);bar_flush(&mut m).unwrap();
 assert_eq!(m.writes,vec![(0x70000,1)]);assert_eq!(m.delayed,0,"a short drain must recheck status before a 100-us sleep");}
 let mut m=Model::new(17);bar_flush(&mut m).unwrap();assert_eq!(m.delayed,100,"longer drain retains the ordinary polling fallback");
 let mut stuck=Model::new(usize::MAX);assert_eq!(bar_flush(&mut stuck),Err("display register wait timed out"));
 assert_eq!(stuck.delayed,2_000_000,"retain the existing 20000 by 100-us timeout budget");
 assert_eq!(stuck.writes,vec![(0x70000,1)]);
}
'''.replace('WAIT', item(source, 'pub fn wait(')).replace('FLUSH', item(source, 'pub fn bar_flush('))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-bar-flush-') as tmp:
            p=Path(tmp);(p/'flush.rs').write_text(code)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'flush.rs'),'-o',str(p/'flush')],check=True)
            subprocess.run([str(p/'flush')],check=True)


if __name__ == '__main__':
    unittest.main()
