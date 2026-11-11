"""Run save-open's production control flow against a single-use session fixture."""
from pathlib import Path
import re
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class SaveSession(unittest.TestCase):
    def test_both_formats_reopen_in_new_processes_under_one_session(self):
        source = (ROOT / 'userspace/programs/src/guiapps_suite/guskit.rs').read_text()
        case = item(source, 'pub fn save_open()')
        # This fixture checks process/session ownership, not semantic-tree values.
        # The guest still evaluates those predicates and exact canvas digests.
        case = re.sub(r'\.wait_node\([^\n]+\)\?;', '.wait_node()?;', case)
        case = case.replace('std::fs::remove_file(file)', 'remove_file(file)')
        run_rust(self, r'''
use std::sync::Mutex;
type CaseResult = Result<(), String>;
enum Modifier { Ctrl }
struct Expected;
impl Expected { fn white() -> Self { Self } fn horizontal(&mut self, _: i32, _: i32, _: i32) {} }
#[derive(Default)]
struct State { consumed: bool, live: bool, saved: Vec<String>, reopened: Vec<String>, exits: usize, finished: bool }
static STATE: Mutex<State> = Mutex::new(State { consumed:false,live:false,saved:Vec::new(),reopened:Vec::new(),exits:0,finished:false });
pub struct App { file: String }
impl App {
    fn launch(argv: &[&[u8]], _: &str) -> Result<Self, String> {
        { let mut s = STATE.lock().unwrap();
          if s.consumed { return Err("case did not declare a session".into()); }
          s.consumed = true; }
        let mut app = Self { file:String::new() }; app.launch_next(argv, "Gus Kit")?; Ok(app)
    }
    fn launch_next(&mut self, argv: &[&[u8]], _: &str) -> Result<(), String> {
        let mut s = STATE.lock().unwrap();
        if s.live || s.finished { return Err("previous process/session still owns window".into()); }
        self.file = String::from_utf8(argv[3].to_vec()).unwrap();
        if s.saved.contains(&self.file) { s.reopened.push(self.file.clone()); }
        s.live = true; Ok(())
    }
    fn wait_node(&mut self) -> Result<(), String> { Ok(()) }
    fn key(&mut self, code: u16, _: &[Modifier]) -> Result<(), String> {
        if code == 31 { STATE.lock().unwrap().saved.push(self.file.clone()); } Ok(())
    }
    pub fn key_exit(&mut self, _: u16, status: i32) -> Result<(), String> {
        assert_eq!(status, 0); let mut s=STATE.lock().unwrap(); assert!(s.live); s.live=false; s.exits+=1; Ok(())
    }
    fn finish(self) -> Result<(), String> { let mut s=STATE.lock().unwrap(); s.live=false; s.finished=true; Ok(()) }
}
fn remove_file(_: &str) -> std::io::Result<()> { Ok(()) }
fn stroke(_: &mut App, _: i32) -> Result<(), String> { Ok(()) }
fn expect(_: &mut App, _: &Expected) -> Result<(), String> { Ok(()) }
CASE
#[test] fn round_trips() {
    save_open().unwrap();
    let s=STATE.lock().unwrap();
    assert_eq!(s.saved, ["/tmp/a.bmp", "/tmp/a.png"]);
    assert_eq!(s.reopened, s.saved);
    assert_eq!(s.exits, 4, "each writer and reader must exit before the next launch");
    assert!(s.finished && !s.live);
}
'''.replace('CASE', case))


if __name__ == '__main__':
    unittest.main()
