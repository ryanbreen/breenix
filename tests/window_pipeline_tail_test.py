"""The bounded read-side trace retains the input window amid other animators."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class PipelineTail(unittest.TestCase):
    def test_other_windows_do_not_erase_input_stage_evidence(self):
        with tempfile.TemporaryDirectory(prefix='pipeline-tail-') as directory:
            p = Path(directory)
            pipeline = (ROOT / 'kernel/src/window/pipeline.rs').read_text().replace('//!', '//')
            (p / 'check.rs').write_text(f'''extern crate alloc;
mod spin {{
 pub struct Mutex<T>(std::sync::Mutex<T>);
 impl<T> Mutex<T> {{pub const fn new(value:T)->Self{{Self(std::sync::Mutex::new(value))}}
 pub fn lock(&self)->std::sync::MutexGuard<'_,T>{{self.0.lock().unwrap()}}}}
}}
mod events {{pub fn now_ns()->u64 {{1}}}}
mod pipeline {{use crate::spin;
{pipeline}
}}
fn main() {{
 pipeline::record(1,7,100,777); pipeline::record(2,7,9,777);
 pipeline::record(3,42,0,777); pipeline::record(4,42,0,777);
 for n in 0..1000 {{pipeline::record(8,99,n,0);}}
 pipeline::record(8,42,12,777); pipeline::record(9,42,12,50);
 pipeline::record(10,50,7,0); pipeline::record(11,50,7,9); pipeline::record(12,50,7,9);
 let text=pipeline::text();let rows:Vec<Vec<u64>>=text.lines().map(|r|r.split_whitespace().map(|v|v.parse().unwrap()).collect()).collect();
 assert!(rows.len()<=256);
 for stage in 1..=4 {{assert!(rows.iter().any(|r|r[0]==stage && r[3]==777),"input stage {{stage}} erased by other clients");}}
 for stage in 8..=12 {{assert!(rows.iter().any(|r|r[0]==stage));}}
}}
''')
            compiler = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '-Dwarnings']
            subprocess.run(compiler + [str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
