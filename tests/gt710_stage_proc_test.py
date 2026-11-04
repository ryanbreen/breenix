"""Native stage proc routes expose producer records without changing the runner."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class StageProc(unittest.TestCase):
    def test_native_stage_routes(self):
        source = (ROOT / 'kernel/src/fs/procfs/mod.rs').read_text()
        for entry in ('BootRibbon', 'StageComposites'):
            body = item(source, '        ProcEntryType::' + entry + ' => {')
            self.assertNotIn('Err(-38)', body, entry + ' must expose its native producer')
        capture = item((ROOT / 'kernel/src/log_buffer.rs').read_text(), 'pub fn capture_byte(')
        self.assertIn('crate::boot_ribbon::byte(byte)', capture)

    def test_metal_catalogue_and_producer(self):
        source = (ROOT / 'kernel/stage_markers.rs').read_text()
        self.assertIn('pub fn metal_ribbon(', source, 'native stage catalogue is missing')
        path = json.loads((ROOT / 'docs/metal/boot-path.json').read_text())
        catalog = json.loads((ROOT / 'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json').read_text())
        markers = {s['name']: s['marker'] for s in catalog['stages']}
        expected = [markers[s] if isinstance(s, str) else s['marker']
                    for m in path['milestones'] for s in m.get('stages', {}).get('x86_64', [])
                    if isinstance(s, str) or 'marker' in s]
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-stage-proc-') as tmp:
            p = Path(tmp)
            generator = '''extern crate alloc;
#[path="ROOT/kernel/stage_markers.rs"] pub mod stage_markers;
fn main(){let markers=stage_markers::metal_ribbon(std::path::Path::new("ROOT")).unwrap();
for m in &markers {println!("{m}");}
std::fs::write("OUT/boot_ribbon.rs",format!("&{markers:?}")).unwrap();}
'''.replace('ROOT', str(ROOT)).replace('OUT', str(p))
            (p / 'catalog.rs').write_text(generator)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'catalog.rs'), '-o', str(p / 'catalog')], check=True)
            self.assertEqual(subprocess.check_output([str(p / 'catalog')], text=True).splitlines(), expected)
            producer = '''extern crate alloc;
extern crate self as spin;
pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T>{pub const fn new(v:T)->Self{Self(std::sync::Mutex::new(v))}
pub fn try_lock(&self)->Option<std::sync::MutexGuard<'_,T>>{
if BUSY.load(std::sync::atomic::Ordering::SeqCst){None}else{self.0.try_lock().ok()}}}
static BUSY:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
static IRQ:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(true);
pub fn arch_interrupts_enabled()->bool{IRQ.load(std::sync::atomic::Ordering::SeqCst)}
pub mod time {pub fn get_monotonic_time()->u64{1234}}
pub mod console {#[path="ROOT/kernel/src/console/status.rs"] pub mod status;}
#[path="ROOT/kernel/src/graphics/boot_producer.rs"] pub mod boot_producer;
#[path="ROOT/kernel/src/graphics/boot_ribbon.rs"] pub mod boot_ribbon;
fn main(){
for b in b"will emit [metal] kernel idle\\n"{boot_ribbon::byte(*b);}
assert!(!boot_ribbon::records().contains("1|[metal] kernel idle"));
for b in b"[metal] kernel idle\\n"{boot_ribbon::byte(*b);}
assert!(boot_ribbon::records().contains("1|[metal] kernel idle"));
assert_eq!(boot_ribbon::serial().unwrap(),"will emit [metal] kernel idle\\n[metal] kernel idle\\n");
// Model the x86 masked syscall and a preempted render-thread scanner.
IRQ.store(false,std::sync::atomic::Ordering::SeqCst);
BUSY.store(true,std::sync::atomic::Ordering::SeqCst);
assert!(!boot_ribbon::drain());
for _ in 0..9000 { boot_ribbon::byte(b'x'); }
for b in b"\\n[xhci] running: \\n" { boot_ribbon::byte(*b); }
assert!(boot_ribbon::records().contains("1|[xhci] running: "));
IRQ.store(true,std::sync::atomic::Ordering::SeqCst);
assert!(boot_ribbon::reached().into_iter().any(|v|v));
BUSY.store(false,std::sync::atomic::Ordering::SeqCst);
for &b in b"SUITE console START cases=2\\nSUITE console CASE text/a PASS ms=1\\nSUITE console CASE text/b FAIL ms=1 msg=bad\\n" {boot_ribbon::byte(b);}
boot_ribbon::records();
let suite=boot_ribbon::suite_status().unwrap();
assert_eq!((suite.passed(),suite.failed(),suite.pending()),(1,1,0));
assert_eq!(suite.elapsed(6234),(0,5));
assert!(boot_ribbon::records().contains("1|[xhci] running: "));
}
'''.replace('ROOT', str(ROOT))
            (p / 'producer.rs').write_text(producer)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'producer.rs'), '-o', str(p / 'producer')], check=True, env=dict(os.environ, OUT_DIR=str(p)))
            subprocess.run([str(p / 'producer')], check=True)


if __name__ == '__main__':
    unittest.main()
