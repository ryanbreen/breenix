"""The x86 diagnostic producer retries a refused try_print without inventing bytes."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class DiagnosticRetry(unittest.TestCase):
    def test_x86_try_print_producer(self):
        source = (ROOT / 'kernel/src/test_control.rs').read_text()
        emit = item(source, 'pub fn emit_diagnostic(')
        self.assertIn('emit_try_print(text)', emit,
                      'diagnostic producer ignores a refused try_print')
        helper = item(source, 'fn emit_try_print(')
        code = r'''
use std::cell::Cell;
thread_local!{static NOW:Cell<u64>=const{Cell::new(0)};static CALLS:Cell<usize>=const{Cell::new(0)};static REFUSE:Cell<usize>=const{Cell::new(0)};static BYTES:Cell<usize>=const{Cell::new(0)};}
mod signal{pub fn monotonic_nanos()->u64{crate::NOW.with(|n|{let v=n.get();n.set(v+1_000_000);v})}}
mod task{pub mod scheduler{pub fn yield_current(){}}}
mod serial{pub fn try_print(a:std::fmt::Arguments<'_>)->Result<(),()>{
let call=crate::CALLS.with(|c|{let n=c.get();c.set(n+1);n});
if call<crate::REFUSE.with(std::cell::Cell::get){return Err(());}
crate::BYTES.with(|b|b.set(b.get()+a.to_string().len()));Ok(())}}
HELPER
fn main(){for (refuse,calls,bytes) in [(0,1,32),(2,3,32),(usize::MAX,101,0)]{
NOW.with(|n|n.set(0));CALLS.with(|c|c.set(0));BYTES.with(|b|b.set(0));REFUSE.with(|r|r.set(refuse));
emit_try_print("BREENGEL-EMIT-DIAGNOSTIC-PAYLOAD");
assert_eq!(CALLS.with(Cell::get),calls);assert_eq!(BYTES.with(Cell::get),bytes);
}}
'''.replace('HELPER', helper)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-diagnostic-') as tmp:
            p = Path(tmp)
            (p / 'retry.rs').write_text(code)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'retry.rs'), '-o', str(p / 'retry')], check=True)
            subprocess.run([str(p / 'retry')], check=True)


if __name__ == '__main__':
    unittest.main()
