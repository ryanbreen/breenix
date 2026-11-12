"""Execute the frame diagnostic collector and check bterm's measured boundaries."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class FrameDiagnostic(unittest.TestCase):
    def test_frame_stages_include_cpu_and_run_queue_samples(self):
        source = (ROOT / 'kernel/src/window/pipeline.rs').read_text()
        self.assertIn('pub(super) fn frame_stage(', source,
                      'frame stage collector is missing')
        collector = item(source, 'pub(super) fn frame_stage(').replace('pub(super)', 'pub')
        harness = r'''
use std::sync::Mutex;
static ROWS:Mutex<Vec<[u64;5]>>=Mutex::new(Vec::new());
mod events { pub fn now_ns()->u64 {900} }
mod task {
 pub mod thread { pub fn current_thread_cpu_split_ns()->Option<(u64,u64)>{Some((100,20))} }
 pub mod scheduler {
  pub static BUSY:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
  pub fn current_cpu()->usize{2}
  pub struct Snapshot {pub per_cpu_ready_len:[u64;8], pub per_cpu_current:[u64;8]}
  pub fn try_liveness_snapshot(cpu:usize)->Option<Snapshot>{
   assert_eq!(cpu,2);
   if BUSY.load(std::sync::atomic::Ordering::Relaxed){return None;}
   Some(Snapshot{per_cpu_ready_len:[1,2,3,4,0,0,0,0],per_cpu_current:[11,12,13,14,0,0,0,0]})
  }
 }
}
mod pipeline {
 fn at(stage:u64,object:u64,related:u64,source:u64,ns:u64){super::ROWS.lock().unwrap().push([stage,object,related,source,ns]);}
 COLLECTOR
}
fn main(){
 let frame=breengel_abi::FrameStage{id:9,frame:7,stage:breengel_abi::frame_stage::TERMINAL,reserved:0};
 pipeline::frame_stage(&frame);
 let rows=ROWS.lock().unwrap().clone();
 assert!(rows.contains(&[23,9,7,120,900]),"terminal boundary lacks thread CPU time: {rows:?}");
 assert!(rows.contains(&[29,9,7,3,900]),"CPU2 queue length missing: {rows:?}");
 assert!(rows.contains(&[37,9,7,13,900]),"CPU2 current thread missing: {rows:?}");
 assert!(rows.contains(&[44,9,7,2,900]),"sampling CPU missing: {rows:?}");
 assert_eq!(rows.len(),18,"frame snapshot must stay bounded");
 ROWS.lock().unwrap().clear();
 task::scheduler::BUSY.store(true,std::sync::atomic::Ordering::Relaxed);
 pipeline::frame_stage(&frame);
 let rows=ROWS.lock().unwrap().clone();
 assert_eq!(rows,vec![[23,9,7,120,900],[44,9,7,2,900],[43,9,7,2,900]],"busy snapshot must be explicit, not a zero queue");
}
'''.replace('COLLECTOR', collector)
        with tempfile.TemporaryDirectory(prefix='window-frame-') as directory:
            path = Path(directory)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            rustc = priority + ['rustc', '--edition=2021', '-Dwarnings']
            subprocess.run(rustc + ['--crate-name', 'breengel_abi', '--crate-type', 'rlib',
                str(ROOT / 'libs/breengel-abi/src/lib.rs'), '--out-dir', str(path)], check=True)
            (path / 'check.rs').write_text(harness)
            subprocess.run(rustc + ['--extern', f'breengel_abi={path / "libbreengel_abi.rlib"}',
                str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
            subprocess.run([str(path / 'check')], check=True)

    def test_bterm_records_the_frame_before_and_after_each_draw_stage(self):
        source = (ROOT / 'userspace/programs/src/bterm.rs').read_text()
        begin = source.index('        if need_redraw {')
        frame = source[begin:source.index('        } else {\n            // Nothing to draw', begin)]
        boundaries = ['frame_stage::BEGIN', 'win.surface()', 'frame_stage::ACQUIRED',
            'tab_bar.draw(', 'frame_stage::TABS', 'tab.emu.render(', 'frame_stage::TERMINAL',
            'terminal_node::terminal_value(', 'win.set_tree(', 'frame_stage::TREE',
            'frame_stage::PRESENT_BEGIN', 'win.present()', 'frame_stage::PRESENT_END']
        positions = []
        for boundary in boundaries:
            self.assertIn(boundary, frame, f'missing measured boundary: {boundary}')
            positions.append(frame.index(boundary))
        self.assertEqual(positions, sorted(positions), 'frame stage labels must match the work they measure')
        self.assertIn('record(45,', (ROOT / 'kernel/src/window/ops.rs').read_text(), 'pending GPU acceptance must be timed')
        self.assertIn('record(46,', (ROOT / 'kernel/src/window/ops.rs').read_text(), 'seal decision must be timed')
        desktop = (ROOT / 'userspace/programs/src/guiapps_suite/desktop.rs').read_text()
        self.assertIn('/proc/breenix/window-pipeline-full', desktop)


if __name__ == '__main__':
    unittest.main()
