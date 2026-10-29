"""Exercise live DMA ownership through moves, retention and final release."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DmaOwner(unittest.TestCase):
    def test_live_owner_follows_the_allocation(self):
        source = (ROOT / 'kernel/src/gpu/dma_owner.rs').read_text()
        harness = r'''
extern crate alloc;
extern crate self as spin;
pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
 pub const fn new(v:T)->Self {Self(std::sync::Mutex::new(v))}
 pub fn lock(&self)->std::sync::MutexGuard<'_,T>{self.0.lock().unwrap()}
}
mod owner { SOURCE }
fn main() {
 let a=owner::Allocation::new(8013,"Pages").unwrap();
 a.identify("native-resource",731,27);
 let b=owner::Allocation::new(17,"GR-channel").unwrap();
 b.identify("GR-channel",0,1);
 let s=owner::census();
 assert!(s.contains("pages=8030 allocations=2"),"{s}");
 assert!(s.contains("owner=native-resource resource=731 context=27 pages=8013"),"{s}");
 assert!(s.contains("owner=GR-channel resource=0 context=1 pages=17"),"{s}");
 assert!(s.contains("site="),"allocation caller missing: {s}");
 let retained=std::mem::ManuallyDrop::new(a);
 drop(b);
 assert!(owner::census().contains("pages=8013 allocations=1"));
 // A failed hardware drain retains this guard just as it retains the pages.
 let mut moved=retained;
 unsafe {std::mem::ManuallyDrop::drop(&mut moved)};
 assert!(owner::census().contains("pages=0 allocations=0"));
 // gpu.md's bounded-validation case snapshots 16 MiB in individual pages,
 // while the native channel and runner already own allocations.
 let mut live=Vec::new();
 for _ in 0..4097 {live.push(owner::Allocation::new(1,"stream-snapshot").expect("diagnostic metadata refused the contract's stream"));}
 let census=owner::census();
 assert!(census.lines().count()<=4097,"owner census is not bounded");
 let n=live.len();drop(live.pop());
 live.push(owner::Allocation::new(2,"reused").unwrap());
 assert!(owner::census().contains(&format!("pages={} allocations={}",n+1,n)));
 drop(live);
 assert!(owner::census().contains("pages=0 allocations=0"));
}
'''.replace('SOURCE', source)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'owner.rs').write_text(harness)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'owner.rs'), '-o', str(p / 'owner')], check=True)
            subprocess.run([str(p / 'owner')], check=True)


if __name__ == '__main__':
    unittest.main()
