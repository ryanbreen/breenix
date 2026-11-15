"""Exercise the production display loop with an owner that refuses its resize."""
from pathlib import Path
import subprocess
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]

class DisplayResize(unittest.TestCase):
    def test_refused_resize_keeps_content_frame_and_does_not_pin_later_moves(self):
        source = (ROOT / 'userspace/programs/src/bwm.rs').read_text()
        loop = item(source, '    for ((window, planned), previous)' if '    for ((window, planned), previous)' in source else '    for (window, planned) in windows.iter_mut().zip(plan.windows)')
        retain = item(source, 'fn retain_display_resizes(') if 'fn retain_display_resizes(' in source else ''
        harness = r'''
mod placement {
    #[derive(Clone, Copy)] pub struct Rect { pub x:i32, pub y:i32, pub width:usize, pub height:usize }
    #[derive(Clone, Copy)] pub struct Frame { pub rect:Rect, pub decoration:[usize;2] }
}
mod switch_plan { pub struct WindowPlan { pub frame:super::placement::Frame, pub resize:Option<[u32;2]> } }
#[derive(Default)] struct Queue; impl Queue { fn begin(&mut self) {} }
struct Window { window_id:u64, x:i32, y:i32, width:usize, height:usize, resize_queue:Queue,
    resize_right:Option<i32>, resize_bottom:Option<i32>, frame_checked:bool, check_moved:u32, check_after_ms:u64 }
impl Window { fn content_width(&self)->usize { self.width-4 } fn content_height(&self)->usize { self.height-36 } }
enum Input { Resize { width:u32, height:u32 } }
mod input { pub fn send_id(_:u64, input:super::Input) { let super::Input::Resize {width,height}=input; assert_eq!([width,height],[600,400]); } }
fn monotonic_ms()->Option<u64> { Some(0) }
SOURCE
#[test] fn refused_owner() {
    let mut windows=vec![Window {window_id:1,x:10,y:40,width:304,height:236,resize_queue:Queue,
        resize_right:None,resize_bottom:None,frame_checked:true,check_moved:0,check_after_ms:0}];
    let mut pending:Vec<(u64,switch_plan::WindowPlan)>=Vec::new();
    struct Plan { windows:Vec<switch_plan::WindowPlan> }
    let plan=Plan {windows:vec![switch_plan::WindowPlan {frame:placement::Frame {rect:placement::Rect {x:20,y:80,width:604,height:436},decoration:[4,36]},resize:Some([600,400])}]};
    let frames=vec![placement::Frame {rect:placement::Rect {x:10,y:40,width:304,height:236},decoration:[4,36]}];
    LOOP
    assert_eq!((windows[0].width,windows[0].height),(304,236),"frame must match the refusing owner's published content immediately");
    // A scene refresh reports the unchanged buffer, then the user moves it.
    windows[0].width=304; windows[0].height=236;
    RECONCILE
    windows[0].x=90; windows[0].y=100;
    RECONCILE
    assert_eq!((windows[0].x,windows[0].y,windows[0].width,windows[0].height),(90,100,304,236));
    assert!(pending.is_empty(),"a refused resize must retire without retry");
}
'''.replace('SOURCE', (item(source, 'fn apply_frame(') if 'fn apply_frame(' in source else '') + '\n' + retain).replace('LOOP', loop).replace('RECONCILE', 'retain_display_resizes(&mut windows, &mut pending);' if retain else 'pending.clear();')
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'test.rs').write_text(harness)
            for cmd in [['taskpolicy','-b','rustc','--edition=2021','--test',str(path/'test.rs'),'-o',str(path/'test')],[str(path/'test')]]:
                result=subprocess.run(cmd,capture_output=True,text=True,cwd=ROOT)
                self.assertEqual(result.returncode,0,result.stdout+result.stderr)

if __name__ == '__main__': unittest.main()
