"""Run BWM's startup selection and production switch plan at a native mode."""
from pathlib import Path
import subprocess
import tempfile
import unittest
from gt710_gr_live_unwind_test import item
ROOT=Path(__file__).resolve().parents[1]

class StartupScale(unittest.TestCase):
    def test_native_startup_and_first_switch_share_kernel_scale(self):
        source=(ROOT/'userspace/programs/src/bwm.rs').read_text()
        config=(ROOT/'userspace/programs/src/bwm/config.rs').read_text()
        start=source.index('    replace_ui(',source.index('let mut screen_h'))
        choice=source[start:source.index(';',start)+1]
        methods=item(config,'    pub(super) fn for_scale(')
        if 'fn for_display(' in config: methods+=item(config,'    pub(super) fn for_display(')
        harness=r'''
mod config { pub const TASKBAR_HEIGHT:usize=28; pub const APPBAR_HEIGHT:usize=36; }
#[path="PLACEMENT"] mod placement;
#[path="SWITCH"] mod switch_plan;
mod bitmap_font {
    #[derive(Clone,Copy)] pub enum RasterHeight { Size16,Size32 }
    pub struct Metrics {pub char_width:usize, height:usize}
    impl Metrics {pub fn line_height(&self)->usize {self.height}}
    pub fn metrics_at(r:RasterHeight)->Metrics {match r {RasterHeight::Size16=>Metrics{char_width:8,height:16},RasterHeight::Size32=>Metrics{char_width:16,height:32}}}
}
const MIN_TEXT_COLS:usize=128; const MIN_TEXT_ROWS:usize=36;
struct Ui {scale:usize,font:bitmap_font::RasterHeight,cell_w:usize,cell_h:usize,mode:switch_plan::Mode,layout:placement::Layout}
impl Ui { METHODS }
thread_local! {static UI:std::cell::RefCell<Option<Ui>>=const {std::cell::RefCell::new(None)};}
fn replace_ui(ui:Ui) {UI.with(|u| *u.borrow_mut()=Some(ui));}
#[test] fn native_first_switch() {
    let configured=false;
    let mode=switch_plan::Mode {width:2560,height:1440,scale:1};
    let screen_w=mode.width; let screen_h=mode.height;
    CHOICE
    UI.with(|u| {
        let old=u.borrow().as_ref().unwrap().mode;
        let frames=[placement::Frame {rect:placement::Rect{x:30,y:100,width:304,height:236},decoration:[4,36]}];
        let plan=switch_plan::switch_plan(old,Ok(switch_plan::Mode{width:1920,height:1080,scale:1}),&frames);
        assert_eq!(plan.windows[0].frame.rect.width,304,"same kernel scale must keep window width");
        assert_eq!(plan.windows[0].frame.rect.height,236,"same kernel scale must keep window height");
        assert_eq!(old.scale,1);
    });
}
'''.replace('PLACEMENT',str(ROOT/'userspace/programs/src/bwm/placement.rs')).replace('SWITCH',str(ROOT/'userspace/programs/src/bwm/switch_plan.rs')).replace('METHODS',methods.replace('pub(super) ','')).replace('CHOICE',choice)
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory); (path/'test.rs').write_text(harness)
            for cmd in [['taskpolicy','-b','rustc','--edition=2021','--test',str(path/'test.rs'),'-o',str(path/'test')],[str(path/'test')]]:
                r=subprocess.run(cmd,capture_output=True,text=True,cwd=ROOT)
                self.assertEqual(r.returncode,0,r.stdout+r.stderr)
if __name__=='__main__': unittest.main()
