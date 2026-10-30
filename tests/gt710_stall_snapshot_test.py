"""Capture every active native SM and the live FIFO state before reset."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class StallSnapshot(unittest.TestCase):
    def test_first_refusing_registers_survive_zero_reads_until_worker_takes_them(self):
        source = (ROOT / 'kernel/src/gpu/gk208/gr/channel.rs').read_text()
        capture = source[source.index('    const REFUSAL_REGISTERS:'):source.index('    /// Keep the words')]
        harness = r'''
struct Registers([u32;7]);
impl Registers {fn read(&mut self,r:usize)->u32 {self.0[REFUSAL_REGISTERS.iter().position(|&v|v==r).unwrap()]}}
// CAPTURE
fn main(){
 let mut regs=Registers([10,11,12,13,14,15,16]);
 save_refusal(&mut regs,[1,2,3,4]);
 regs.0=[0;7];save_refusal(&mut regs,[0;4]);
 assert_eq!(take_refusal(),Some(([1,2,3,4],[10,11,12,13,14,15,16])));
 assert_eq!(take_refusal(),None);
 regs.0=[20;7];save_refusal(&mut regs,[5,6,7,8]);
 assert_eq!(take_refusal(),Some(([5,6,7,8],[20;7])));
}
'''.replace('// CAPTURE', capture)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'capture.rs').write_text(harness)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(path / 'capture.rs'), '-o', str(path / 'capture')], check=True)
            subprocess.run([str(path / 'capture')], check=True)

    def test_two_engine_faults_and_forced_reset_capture_once_outside_core(self):
        adapter = (ROOT / 'kernel/src/gpu/gk208/adapter.rs').read_text()
        # Run the health-fault path, stopping before completion sampling, and
        # the complete reset entry with its native diagnostics enabled.
        reap = item(adapter, '    pub fn reap(')
        reap = reap[:reap.index('        let Some(front)')] + '        Reap::Empty\n    }'
        begin = item(adapter, '    pub fn begin_reset(')
        methods = '\n'.join([reap, begin, item(adapter, '    pub fn fault_reason('),
                             item(adapter, '    fn retry_error_trap(')])
        methods = methods.replace('#[cfg(target_os = "none")]', '')
        harness = r'''
use std::{cell::RefCell, rc::Rc, collections::VecDeque};
macro_rules! serial_println {($($arg:tt)*)=>{let _=format_args!($($arg)*);};}
pub(crate) use serial_println;
pub struct Done {pub tag:u32,pub fence:Option<u64>,pub native_fence:Option<u64>,pub answer:u32,pub echoed:u64,pub fenced:bool}
pub enum Reap { Empty, Fault, Done(Done), Reset(u32) }
struct Request {tag:u32}
#[derive(Default)]
struct State { in_core:bool, records:Vec<(u64,u32,bool,String)>, resets:usize }
struct Channel { state:Rc<RefCell<State>>, epoch:u64, put:u32, fault:bool }
impl Channel {
    fn healthy(&self)->Result<(), &'static str> {
        if self.fault { Err("injected engine fault") } else { Ok(()) }
    }
    fn record_fault(&self, reason:&str) {
        let mut s=self.state.borrow_mut();
        let locked=s.in_core;
        s.records.push((self.epoch,self.put,locked,reason.into()));
    }
    fn acknowledge_error(&mut self,_:(),_:u32)->Result<(), &'static str>{panic!("generic health test has no attributed MMU fault")}
    fn reset(&mut self)->Result<(), &'static str> {
        let mut s=self.state.borrow_mut();
        assert_eq!(s.records.last().map(|r|(r.0,r.1)),Some((self.epoch,self.put)),
                   "capture must precede hardware reset");
        s.resets+=1;Ok(())
    }
}
#[derive(Clone,Copy)] struct Identity { sequence:u32, epoch:u64 }
struct Sent { identity:Option<Identity> }
struct Trace {
    sequence:u32,epoch:u64,tag:u32,channel:u64,caller:u64,kind:&'static str,
    target:u64,frame:u32,address:u64,command:u32,
}
struct Device {
    retired:VecDeque<(Request,Option<u32>)>,attributed:Option<((),u32)>,channel:Channel,pri_pending:bool,fault:bool,first_error:Option<&'static str>,
    resetting:Option<Result<(), &'static str>>,recent:Vec<Trace>,pending:Vec<Sent>,
}
impl Device {
    // This fixture injects generic channel health failures, with no submitted
    // undefined method. The adapter lifecycle harness covers attributed traps.
    fn retire_error_trap(&mut self)->Result<bool, &'static str> {
        assert!(self.pending.is_empty(), "generic fault fixture queued work");
        Ok(false)
    }
    // METHODS
}
fn main() {
    let state=Rc::new(RefCell::new(State::default()));
    for (epoch,put,fault) in [(17,23,true),(18,23,true),(19,24,false)] {
        let mut d=Device { retired:VecDeque::new(),attributed:None,channel:Channel {state:state.clone(),epoch,put,fault},
            pri_pending:false,fault:false,first_error:None,resetting:None,recent:vec![],pending:vec![] };
        state.borrow_mut().in_core=true;
        for _ in 0..2 {
            assert_eq!(matches!(d.reap(),Reap::Fault),fault);
        }
        state.borrow_mut().in_core=false;
        d.begin_reset();
        d.begin_reset();
    }
    let s=state.borrow();
    assert_eq!(s.records.iter().map(|r|(r.0,r.1)).collect::<Vec<_>>(),
               [(17,23),(18,23),(19,24)],"one pre-reset record per (epoch, put)");
    assert!(s.records.iter().all(|r|!r.2),"capture ran under CORE");
    assert_eq!(s.records.iter().map(|r|r.3.as_str()).collect::<Vec<_>>(),
               ["injected engine fault","injected engine fault","shared core requested reset"]);
    assert_eq!(s.resets,3);
}
'''.replace('// METHODS', methods)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'faults.rs').write_text(harness)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings',
                           str(path / 'faults.rs'), '-o', str(path / 'faults')], check=True)
            subprocess.run([str(path / 'faults')], check=True)

    def test_stall_bulk_record_requires_its_exact_serial_anchor(self):
        import hashlib
        import runpy
        sys.path.insert(0, str(ROOT / 'scripts/metal'))
        api = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
        startup = b'GT710_PIX 0 1:ff003039\n'
        record = b'GT710_STALL_V1 instance=8192 epoch=17 put=23 registers=[(4196100, 2415924756)]\n'
        anchor = (f'GT710_STALL_BULK epoch=17 put=23 bytes={len(record)-1} '
                  f'sha256={hashlib.sha256(record[:-1]).hexdigest()}\n')
        serial = ('GT710_MODE {"schema":1}\n'
                  f'GT710_BULK {len(startup)} {hashlib.sha256(startup).hexdigest()}\n'
                  'SUITE gpu START total=1\nSUITE gpu CASE pipelines/sample FAIL ms=1 msg=EIO\n'
                  + anchor + 'SUITE gpu DONE passed=0 failed=1 skipped=0 total=1\n'
                  'SUITE gpu MEMORY reclaimed=PASS free_before=100 free_after=100 allowance=48\n'
                  f'GT710_FINAL_BULK {len(record)} {hashlib.sha256(record).hexdigest()}\n'
                  'SUITE_SEQUENCE READY gpu\n')
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'gt710-pixels.rle').write_bytes(startup)
            (path / 'gt710-final.rle').write_bytes(record)
            (path / 'serial.txt').write_text(serial)
            self.assertIn(record.decode() + anchor, api['capture_serial'](path))
            for bad in [anchor.replace('put=23', 'put=24'),
                        anchor.replace('sha256=', 'sha256=0'),
                        anchor.replace(f'bytes={len(record)-1}', 'bytes=1'), '']:
                (path / 'serial.txt').write_text(serial.replace(anchor, bad))
                with self.assertRaises(ValueError):
                    api['capture_serial'](path)

    def test_second_tpc_and_live_fifo_are_retained_without_selector_writes(self):
        source = f'''
extern crate alloc;
#[path = "{ROOT / 'kernel/src/gpu/gk208/gr/topology.rs'}"] pub mod topology;
#[path = "{ROOT / 'kernel/src/gpu/gk208/gr/stall.rs'}"] mod stall;
fn main() {{
    let raw = [[0x409604,1],[0x502608,2],[0x500c30,3]];
    let topology = topology::decode(&raw).unwrap();
    assert_eq!(topology.tpc_nr[0],2);
    assert_eq!(topology.raw,&raw);
    let mut reads = std::collections::BTreeSet::new();
    let rows = stall::capture(&raw,1,|address| {{
        assert!(reads.insert(address), "duplicate register read");
        if address==0x504e48 {{ 1 }} else if address==0x42084 {{ 0x20000110 }} else {{ 0 }}
    }}).unwrap();
    assert!(rows.contains(&(0x504e48,1)), "TPC1's stack error was omitted");
    for address in [0x504614,0x504618,0x504624,0x504628,0x504650,
                    0x504e14,0x504e18,0x504e24,0x504e28,0x504e50,
                    0x504410,0x504c10,0x42018,0x4205c,0x42084,0x42088,0x42044] {{
        assert!(reads.contains(&address), "missing diagnostic register {{address:#x}}");
    }}
    assert!(rows.contains(&(0x42084,0x20000110)), "live FIFO method was omitted");
    assert!(stall::capture(&[[0x409604,1],[0x502608,2],[0x500c30,1]],1,
        |_| panic!("invalid topology must refuse before MMIO")).is_err());
    assert!(stall::capture(&raw,32,|_| panic!("invalid PBDMA must refuse before MMIO")).is_err());
}}
'''
        prefix = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'snapshot.rs').write_text(source)
            subprocess.run(prefix + ['rustc', '--edition=2021', '-Dwarnings',
                           str(path / 'snapshot.rs'), '-o', str(path / 'snapshot')], check=True)
            subprocess.run([str(path / 'snapshot')], check=True)


if __name__ == '__main__':
    unittest.main()
