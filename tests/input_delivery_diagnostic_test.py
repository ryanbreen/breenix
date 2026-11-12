"""Exercise delivery verdicts and failure-retained publication observations."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


def execute(source):
    with tempfile.TemporaryDirectory(prefix='delivery-diagnostic-') as directory:
        path = Path(directory)
        (path / 'check.rs').write_text(source)
        command = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '-Dwarnings']
        subprocess.run(command + ['--crate-name', 'input_abi', '--crate-type', 'rlib', str(ROOT / 'libs/input-abi/src/lib.rs'), '--out-dir', str(path)], check=True)
        built = subprocess.run(command + ['--extern', f'input_abi={path / "libinput_abi.rlib"}', str(path / 'check.rs'), '-o', str(path / 'check')], capture_output=True, text=True,
                               env=dict(os.environ, CARGO_BUILD_JOBS='4'))
        if built.returncode:
            raise AssertionError(built.stderr)
        return subprocess.run([str(path / 'check')], capture_output=True, text=True)


class DeliveryDiagnostic(unittest.TestCase):
    def test_delivery_requires_publication_even_without_pixel_changes(self):
        pointer = (ROOT / 'libs/libuitest/src/pointer_live.rs').read_text()
        # The historical body can be supplied to this harness during analysis;
        # the committed guard executes the current production body.
        outcome = item(pointer, 'fn outcome(')
        ledger_stub = ''
        if 'read_to_string' in outcome:
            outcome = outcome.replace('std::fs::read_to_string', 'ledger_text')
            ledger_stub = '''
fn ledger_text(_: &str) -> Result<String, String> { Ok("41 delivered 9 none\\n".into()) }
struct Row { disposition: &'static str, window: u64, reason: &'static str }
fn ledger_entry(_: &str, _: u64) -> Result<Option<Row>, String> { Ok(Some(Row { disposition: "delivered", window: 9, reason: "none" })) }
'''
        else:
            ledger_stub = '''
struct Query { disposition: u32, window: u64, reason: u32 }
struct Reader;
impl Reader {
 fn open(_: bool) -> Result<Self,String> { Ok(Self) }
 fn outcome(&self, serial: u64) -> Result<Option<Query>,String> {
  assert_eq!(serial,41); Ok(Some(Query { disposition: 2, window: 9, reason: 0 }))
 }
}
'''
        delivery = item(pointer, 'fn wait_delivery(')
        watchdog = '''struct Watchdog; enum Wait { InputDelivery }
impl Watchdog { fn new(_:Wait)->Result<Self,String> { Ok(Self) } }''' if 'Watchdog' in delivery else ''
        processed = item((ROOT / 'libs/libuitest/src/input.rs').read_text(), 'pub fn client_processed(')
        source = r'''
use std::cell::Cell;
use std::time::Duration;
mod libbreenix { pub mod input { pub use input_abi as abi; } }
thread_local! { static CLOCK: Cell<u64> = const { Cell::new(0) }; static META: Cell<(u64,u64)> = const { Cell::new((1,40)) }; }
#[derive(Clone,Copy,PartialEq,PartialOrd)] struct Instant(u64);
impl Instant { fn now()->Self { Self(CLOCK.with(Cell::get)) } }
impl std::ops::Add<Duration> for Instant { type Output=Self; fn add(self,d:Duration)->Self { Self(self.0+d.as_millis() as u64) } }
fn pause()->Result<(),String> { CLOCK.with(|c|c.set(c.get()+1)); Ok(()) }
WATCHDOG
struct Delivery { serial:u64, window:u64 }
mod window {
 pub struct Meta { pub processed_serial:u64 }
 pub fn read_meta(id:u64)->Result<(u64,Meta),String> { assert_eq!(id,9); let (c,s)=super::META.with(std::cell::Cell::get); Ok((c,Meta { processed_serial:s })) }
}
mod messages { pub fn input(_: &str,_:u64,disposition:&str,detail:&str)->String { format!("{disposition} {detail}") } }
PROCESSED
LEDGER
mod live {
 use super::*;
 OUTCOME
 DELIVERY
 pub fn run() {
  let error=wait_delivery(41,Some(9),false).err().expect("dequeue alone is not processing");
  assert!(error.contains("delivered unprocessed window=Some(9)"),"{error}");
  assert_eq!(CLOCK.with(Cell::get),2000,"delivery deadline changed");
  // A no-change commit publishes the derived source watermark without new pixels.
  META.with(|m|m.set((2,input_abi::derived_serial(41,1).unwrap())));
  let delivery=wait_delivery(41,Some(9),false).unwrap();
  assert_eq!((delivery.serial,delivery.window),(41,9));
  println!("unpublished motion times out at 2000 ms; published no-change ack passes");
 }
}
fn main(){ live::run(); }
'''.replace('PROCESSED', processed).replace('LEDGER', ledger_stub).replace('OUTCOME', outcome).replace('DELIVERY', delivery).replace('WATCHDOG', watchdog)
        result = execute(source)
        self.assertEqual(result.returncode, 0, result.stderr)
        print(result.stdout, end='')

    def test_failure_capture_retains_no_change_and_metadata_reads(self):
        ops = (ROOT / 'kernel/src/window/ops.rs').read_text()
        desktop = (ROOT / 'userspace/programs/src/guiapps_suite/desktop.rs').read_text()
        begin = desktop.index('let trace = trace.lines().filter(')
        selection = desktop[begin:desktop.index(';', begin) + 1]
        source = r'''
use std::sync::Mutex;
#[derive(Clone,Copy)] pub struct Meta { processed_serial:u64 }
pub struct Caller;
pub struct NoChange { reserved:u64, id:u64, commit_id:u64 }
const EINVAL:i32=22;
struct Window { accepted_id:u64,commit_id:u64,meta:Meta }
struct Registry { w:Window }
impl Registry { fn window(&mut self,_:u64,_:&Caller,_:u8)->Result<&mut Window,i32>{Ok(&mut self.w)} }
struct Lock(Mutex<Registry>);
impl Lock { fn lock(&self)->std::sync::MutexGuard<'_,Registry>{self.0.lock().unwrap()} }
static REGISTRY:Lock=Lock(Mutex::new(Registry { w:Window { accepted_id:7,commit_id:7,meta:Meta{processed_serial:40} } }));
struct Gate; impl Gate { fn enter(_:u64,_:&Caller)->Result<Self,i32>{Ok(Self)} }
mod who { pub const owner:u8=1; pub const owner_compositor_or_root:u8=2; }
mod syscall { pub mod window { pub fn copy_out<T>(_:u64,_:&T)->Result<(),i32>{Ok(())} } }
pub mod pipeline {
 pub static ROWS:std::sync::Mutex<Vec<[u64;4]>>=std::sync::Mutex::new(Vec::new());
 pub fn record(stage:u64,id:u64,commit:u64,serial:u64){ROWS.lock().unwrap().push([stage,id,commit,serial]);}
}
mod ops {
 use super::*;
 fn publish_ready(){}
 NO_CHANGE
 READ_META
}
fn selected(trace:&str,start:u64)->String { SELECTION trace }
fn main(){
 let mut n=NoChange {reserved:0,id:9,commit_id:0};
 ops::no_change(&Caller,&mut n,Meta{processed_serial:41},0).unwrap();
 let (commit,meta)=ops::read_meta(&Caller,9).unwrap();
 assert_eq!((commit,meta.processed_serial),(8,41));
 assert_eq!(*pipeline::ROWS.lock().unwrap(),vec![[18,9,8,41],[19,9,8,41]],"no-change and read-meta observations missing");
 let trace="3 9 0 41 100\n4 9 0 41 101\n18 9 8 41 102\n19 9 8 41 103\n17 8 7 1 90\n7 8 1 39 80\n";
 assert_eq!(selected(trace,100),"3 9 0 41 100\n4 9 0 41 101\n18 9 8 41 102\n19 9 8 41 103\n17 8 7 1 90");
}
'''.replace('NO_CHANGE', item(ops, 'pub fn no_change(')).replace('READ_META', item(ops, 'pub fn read_meta(')).replace('SELECTION', selection)
        source = source.replace('mod who {', '#[allow(non_upper_case_globals)] mod who {')
        # The selector before instrumentation does not use a time bound.
        if '>= start' not in selection:
            source = source.replace('fn selected(trace:&str,start:u64)->String {',
                                    'fn selected(trace:&str,start:u64)->String { std::hint::black_box(start);')
        result = execute(source)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
