"""Run the client watermark expressions against mixed input serial namespaces."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class WatermarkOrder(unittest.TestCase):
    def test_handled_sources_advance_past_derived_text(self):
        client = (ROOT / 'userspace/programs/src/wm_client.rs').read_text()
        library = (ROOT / 'libs/breengel/src/window.rs').read_text()
        handled_update = re.search(r'self.handled = ([^;]+);', library).group(1)
        client_update = re.search(r'(?m)^\s*processed = ([^;]+);', client).group(1)
        no_change_update = re.search(r'(?m)^\s*processed = ([^;]+);', library[library.index('fn no_change('):]).group(1)
        handled_update = handled_update.replace('self.handled', 'processed').replace('d.serial', 'e.serial')
        no_change_update = re.sub(r'\bserial\b', 'e.serial', no_change_update)
        with tempfile.TemporaryDirectory(prefix='input-watermark-') as directory:
            p = Path(directory)
            functions = '\n'.join(f'fn update{i}(mut processed:u64,e:Event)->u64 {{processed={body}; processed}}'
                                  for i, body in enumerate([client_update, handled_update, no_change_update]))
            methods = item(library, 'fn no_change(') + '\n' + item(library, 'fn published(')
            code = f'''pub extern crate input_abi;
mod libbreenix {{ pub mod input {{pub use crate::input_abi as abi;}} }}
struct Event {{serial:u64}}
{functions}
type Error=();
#[derive(Clone,Copy,PartialEq)] enum Handled {{NoChange,Redraw}}
#[derive(Clone,Copy)] struct Committed {{commit_id:u64,pixel_gen:u64,processed_serial:u64}}
struct Meta {{processed_serial:u64}}
mod window {{pub fn commit_no_change(_:u64,meta:Option<&super::Meta>)->Result<u64,super::Error>{{
 assert!(meta.is_some());let _=meta.unwrap().processed_serial;Ok(1)
}}}}
struct Window {{id:u64,published:u64,since_pixel:Vec<(u64,Handled)>,last:Option<Committed>}}
impl Window {{
 fn finish_pending(&self)->Result<(),Error>{{Ok(())}}
 fn meta(&self,processed_serial:u64)->Meta{{Meta{{processed_serial}}}}
 {methods}
}}
fn main() {{
 let derived=input_abi::derived_serial(10,1).unwrap();
 for update in [update0,update1,update2] {{
  let mut processed=0;
  for serial in [10,derived,11,12] {{processed=update(processed,Event{{serial}});}}
  assert_eq!(processed,12,"handled raw input remained behind derived text");
  assert_eq!(update(12,Event{{serial:derived}}),12,"an older repeat regressed the source watermark");
  assert_eq!(update(12,Event{{serial:1<<63}}),12,"a kernel release poisoned the source watermark");
 }}
 let redraw=input_abi::derived_serial(11,1).unwrap();
 let mut window=Window{{id:1,published:0,last:None,since_pixel:vec![
  (derived,Handled::NoChange),(11,Handled::NoChange),(redraw,Handled::Redraw),(12,Handled::NoChange)]}};
 assert_eq!(window.no_change().unwrap().processed_serial,11,"NoChange crossed the Redraw barrier");
 window.published(Committed{{commit_id:2,pixel_gen:1,processed_serial:redraw}});
 assert_eq!(window.since_pixel.len(),1,"pixel publication discarded later source input");
 let done=window.no_change().unwrap();
 assert_eq!((done.commit_id,done.pixel_gen,done.processed_serial),(1,1,12));
}}
'''
            (p / 'check.rs').write_text(code)
            compiler = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '-Dwarnings']
            subprocess.run(compiler + ['--crate-type=rlib', '--crate-name=input_abi', str(ROOT / 'libs/input-abi/src/lib.rs'),
                                      '-o', str(p / 'libinput_abi.rlib')], check=True)
            subprocess.run(compiler + [str(p / 'check.rs'), '--extern', f'input_abi={p / "libinput_abi.rlib"}',
                                      '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
