"""Run Gus Kit's document actions against real image files on the host."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


def run_host(test, code):
    with tempfile.TemporaryDirectory() as directory:
        work = Path(directory)
        dependencies = '\n'.join(f'{name} = {{ path = "{ROOT / "libs" / name}" }}'
                                 for name in ['libpixcheck', 'libimg', 'libimg-encode'])
        (work / 'Cargo.toml').write_text('[package]\nname="guskit-review-host"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\n' + dependencies)
        (work / 'src').mkdir()
        (work / 'src/lib.rs').write_text(code)
        command = ['taskpolicy', '-b', 'cargo', 'test', '--offline', '--manifest-path', str(work / 'Cargo.toml')]
        env = dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings',
                   CARGO_TARGET_DIR=str(Path(tempfile.gettempdir()) / 'guskit-review-host-target'))
        result = subprocess.run(command, capture_output=True, text=True, env=env)
        test.assertEqual(result.returncode, 0, result.stdout + result.stderr)


def file_methods(source, names):
    names = list(names) + ['file_busy', 'request_file', 'start_file_io', 'poll_file_io', 'apply_file_result']
    return '\n'.join(item(source, f'fn {name}(') for name in dict.fromkeys(names))


def file_module():
    source = (ROOT / 'userspace/programs/src/guskit/file_io.rs').read_text().replace('//!', '//')
    # These extracted fixtures exercise different subsets of the production request API.
    return '#[allow(dead_code)] mod file_io { use super::libbreenix;\n' + source + '\n}'


DRAIN = """
fn finish_file(app:&mut App) {
    // Drive the event loop's post-frame start and completion poll.
    app.start_file_io();
    while app.file_busy() { app.poll_file_io(); std::thread::yield_now(); }
}
"""


class NewSave(unittest.TestCase):
    def test_new_does_not_replace_opened_image_or_existing_untitled(self):
        source = (ROOT / 'userspace/programs/src/guskit.rs').read_text()
        methods = file_methods(source, ['action', 'open_file', 'save'])
        run_host(self, r'''
#[cfg(test)] mod check {
use std::path::PathBuf;
#[allow(dead_code)] #[path = "CANVAS"] mod canvas;
#[path = "FILES"] mod files;
mod libbreenix { #[allow(dead_code)] #[path = "ATOMIC"] pub mod atomic_file; }
use canvas::{Canvas, History};
use libbreenix::atomic_file as save_barrier;
FILE_MODULE
const HOST_SESSION:u16=8; const JOIN_SESSION:u16=9;
struct Session {core:Core} struct Core;
impl Core {fn submit(&mut self,_:DrawOp) {}}
enum DrawOp {Clear,Undo}
struct SessionDialog;
mod session_dialog {pub enum Mode {Host,Join}}
impl SessionDialog {fn new(_:session_dialog::Mode,_:&())->Self {Self}}
const NEW:u16=1; const QUIT:u16=2; const UNDO:u16=3; const REDO:u16=4; const OPEN:u16=5; const SAVE:u16=6; const SAVE_AS:u16=7;
struct App { file_io:file_io::State,session:Option<Session>,session_dialog:Option<SessionDialog>,layout:(), width:usize, height:usize, history:History, clean:[u8;32], file:Option<PathBuf>, message:String, stroke:Option<Vec<(i32,i32)>> }
impl App {
    fn open_picker(&mut self, _:bool) { panic!("not opening picker"); }
    fn request_quit(&mut self) { panic!("not quitting"); }
    fn can_start_session(&self)->bool {self.session.is_none()}
    fn canvas(&self)->&Canvas {self.history.canvas()}
    METHODS
}
DRAIN
#[test] fn new_draw_save_preserves_photo() {
    let dir=std::env::temp_dir().join(format!("guskit-new-save-{}",std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    std::env::set_var("HOME", &dir);
    let path=dir.join("photo.png");
    let mut image=Canvas::new(8,8); image.pencil(&[(1,1),(3,1)],[12,34,56]);
    let original=files::encode("photo.png",&image,8,8).unwrap();
    std::fs::write(&path,&original).unwrap();
    std::fs::write(dir.join("untitled-1.png"),b"keep existing drawing").unwrap();
    let mut app=App { file_io:file_io::State::default(),session:None,session_dialog:None,layout:(),width:8,height:8,clean:image.digest(),history:History::new(image),file:None,message:String::new(),stroke:None };
    app.open_file(path.clone()); finish_file(&mut app); assert!(app.message.is_empty());
    app.action(NEW);
    let drawn=app.history.canvas().preview(&[(5,5),(7,5)],|c,p|c.pencil(p,[7,8,9]));
    app.history.push(drawn); app.action(SAVE); finish_file(&mut app);
    assert_eq!(std::fs::read(&path).unwrap(),original,"New followed by Save overwrote photo.png");
    assert_eq!(std::fs::read(dir.join("untitled-1.png")).unwrap(),b"keep existing drawing");
    assert_eq!(app.file.as_ref(),Some(&dir.join("untitled-2.png")));
    let saved=std::fs::read(app.file.as_ref().unwrap()).unwrap();
    let (decoded,_)=files::decode("untitled-2.png",&saved,8,8).unwrap();
    assert_eq!(decoded.pixels(),app.history.canvas().pixels());
    assert_eq!(app.message,"Saved");
    std::fs::remove_dir_all(dir).unwrap();
}
}
'''.replace('CANVAS', str(ROOT / 'userspace/programs/src/guskit/canvas.rs'))
   .replace('FILES', str(ROOT / 'userspace/programs/src/guskit/files.rs'))
   .replace('ATOMIC', str(ROOT / 'libs/libbreenix/src/atomic_file.rs'))
   .replace('METHODS', methods).replace('FILE_MODULE', file_module()).replace('DRAIN', DRAIN))


if __name__ == '__main__':
    unittest.main()
