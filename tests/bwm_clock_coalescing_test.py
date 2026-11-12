"""A wall-second repaint must not get ahead of an arriving client frame."""
from pathlib import Path
import unittest
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class ClockCoalescing(unittest.TestCase):
    def test_clock_only_work_uses_the_background_deadline(self):
        source = (ROOT / 'userspace/programs/src/bwm.rs').read_text()
        start = source.index('// ── 4. Update chrome')
        end = source.index('validation.retain_clients', start)
        block = source[start:end]
        run_rust(self, r'''
struct Clock { due: bool }
impl Clock {
 fn due(&self)->bool {self.due}
 fn update_clock(&mut self)->Option<i64> {self.due.then(||{self.due=false;1})}
}
struct Background { drawn: bool }
impl Background {
 fn ready(&mut self,_now:u64,urgent:bool,_pacing:&())->bool {urgent}
 fn reset(&mut self) {self.drawn=true;}
}
struct Gpu { desktop: bool }
fn format_clock(_sec:i64,_text:&mut [u8;11]) {}
fn pacing_ns()->u64 {666_000_000_000}
fn redraw_clock(desktop:&mut bool,_text:&[u8;11],_font:&mut ()) {*desktop=true;}
fn px(n:usize)->usize {n}
const TASKBAR_HEIGHT:usize=28;
fn update(urgent:bool)->(bool,bool,Option<u64>) {
 let mut clock=Clock{due:true};let mut clock_pace=Background{drawn:false};
 let mut gpu=Gpu{desktop:false};let mut clock_text=[0;11];let mut ui_font=();
 let pacing=();let full_redraw=false;let windows_dirty=urgent;
 let mouse_moved_this_frame=false;let cursor_dirty=false;
 let mut content_dirty=false;let mut taskbar_prepare_ns=None;let mut clock_dirty=false;
 let screen_w=1280;let mut dirty_x0=99;let mut dirty_y0=99;
 let mut dirty_x1=0;let mut dirty_y1=0;
 @BLOCK@
 // Exercise the harness policy independently; only the extracted production
 // block decides whether it applies that policy before painting.
 assert!(clock.due() || gpu.desktop);assert!(clock_pace.ready(0,true,&()));
 let drew=clock_pace.drawn;clock_pace.reset();
 std::hint::black_box((full_redraw,windows_dirty,mouse_moved_this_frame,
 cursor_dirty,&pacing,&mut dirty_x0,&mut dirty_y0,&mut dirty_x1,&mut dirty_y1,
 &mut content_dirty,&mut clock_dirty,screen_w,px(TASKBAR_HEIGHT)));
 (gpu.desktop,drew,taskbar_prepare_ns)
}
#[test]fn clock_only_waits_for_the_background_slot(){assert_eq!(update(false),(false,false,None),
 "wall-clock-only repaint entered the synchronous display path before the client commit");}
#[test]fn client_work_absorbs_the_clock_update(){assert_eq!(update(true),(true,true,Some(0)));}
'''.replace('@BLOCK@', block))


if __name__ == '__main__':
    unittest.main()
