"""Exercise the K6 post-EXEC display decision against sticky W1C hardware."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class MemxDisplay(unittest.TestCase):
    def test_post_exec_clear_and_fresh_fault(self):
        fixture = r'''
mod modeset {
    pub trait Io {
        fn read(&mut self, register: usize) -> u32;
        fn write(&mut self, register: usize, value: u32);
    }
}
#[path="@ROOT@/kernel/src/gpu/gk208/memx_display.rs"] mod memx_display;
use memx_display::Health;
use std::collections::BTreeMap;
struct Card { raw:BTreeMap<usize,u32>, writes:Vec<(usize,u32)>, fresh:bool }
impl Card {
    fn new(head:u32,sor:u32) -> Self {
        Self {raw:BTreeMap::from([(0x61009c,0),(0x616308+head as usize*0x800,0x20001),
            (0x61c004+sor as usize*0x800,0x40000001),(0x61c030+sor as usize*0x800,0x20)]),
            writes:Vec::new(),fresh:false}
    }
}
impl modeset::Io for Card {
    fn read(&mut self,r:usize)->u32 {*self.raw.get(&r).expect("wrong health address")}
    fn write(&mut self,r:usize,v:u32) {
        self.writes.push((r,v));
        let sticky=self.raw[&r]&0x10;
        self.raw.insert(r,(v&!0x10)|if v&0x10==0 {sticky}else{0});
        if self.fresh && v&1!=0 {*self.raw.get_mut(&r).unwrap()|=0x10;}
    }
}
#[test] fn raw_registers_and_healthy_reply_do_not_write() {
    let mut c=Card::new(2,3);
    let h=Health::read(&mut c,2,3);
    assert_eq!((h.errors,h.underflow,h.power,h.link),(0,0x20001,0x40000001,0x20));
    assert!(h.check().is_ok());
    let (clear,result)=memx_display::after_exec(&mut c,2,3,h,true);
    assert!(result.is_ok());assert!(clear.is_none());assert!(c.writes.is_empty());
}
#[test] fn isolated_sticky_bit_clears_then_arms_once_for_each_exec() {
    for (head,sor) in [(0,0),(2,3)] {
        let mut c=Card::new(head,sor);let reg=0x616308+head as usize*0x800;
        c.raw.insert(reg,0x20011);
        let h=Health::read(&mut c,head,sor);
        let (clear,result)=memx_display::after_exec(&mut c,head,sor,h,true);
        assert_eq!(c.writes,[(reg,0x20010),(reg,0x20001)]);
        assert!(result.is_ok());assert_eq!(clear.unwrap().underflow,0x20001);
        // A later present/check retains the fresh fault; it has no clear path.
        c.raw.insert(reg,0x20011);
        assert!(Health::read(&mut c,head,sor).check().is_err());
        assert_eq!(c.writes.len(),2);
    }
}
#[test] fn other_terms_and_unarmed_or_red_states_refuse_without_writes() {
    for (register,value) in [(0x61009c,1),(0x61c004,0),(0x61c004,0x80000001),
        (0x61c030,0x10000000),(0x616308,0x10),(0x616308,0x101)] {
        let mut c=Card::new(0,0);c.raw.insert(0x616308,0x11);c.raw.insert(register,value);
        let h=Health::read(&mut c,0,0);
        let (clear,result)=memx_display::after_exec(&mut c,0,0,h,true);
        assert!(result.is_err(),"accepted {register:#x}={value:#x}");
        assert!(clear.is_none());assert!(c.writes.is_empty());
    }
    let mut c=Card::new(0,0);c.raw.insert(0x616308,0x11);
    let h=Health::read(&mut c,0,0);
    let (clear,result)=memx_display::after_exec(&mut c,0,0,h,false);
    assert!(result.is_err());assert!(clear.is_none());assert!(c.writes.is_empty());
}
#[test] fn fresh_underflow_after_clear_is_reported_and_not_cleared_again() {
    let mut c=Card::new(0,0);c.raw.insert(0x616308,0x11);c.fresh=true;
    let h=Health::read(&mut c,0,0);
    let (clear,result)=memx_display::after_exec(&mut c,0,0,h,true);
    assert!(result.is_err());assert_eq!(clear.unwrap().underflow,0x11);
    assert_eq!(c.writes,[(0x616308,0x10),(0x616308,1)]);
}
'''.replace('@ROOT@', str(ROOT))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'display.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                           str(path / 'display.rs'), '-o', str(path / 'display')], check=True)
            subprocess.run([str(path / 'display')], check=True)


if __name__ == '__main__':
    unittest.main()
