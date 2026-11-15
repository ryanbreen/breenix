"""Opening and re-saving a drawing must retain pixels beyond the viewport."""
import unittest
from guskit_new_save_test import ROOT, run_host, file_methods, file_module, DRAIN

class DocumentOpen(unittest.TestCase):
    def test_larger_document_survives_open_and_resave(self):
        source = (ROOT/'userspace/programs/src/guskit.rs').read_text()
        methods = file_methods(source, ['open_file','save_to'])
        run_host(self,r'''
#[cfg(test)] mod check {
use std::path::PathBuf;
#[allow(dead_code)] #[path="CANVAS"] mod canvas;
#[path="FILES"] mod files;
mod libbreenix {#[allow(dead_code)] #[path="ATOMIC"] pub mod atomic_file;}
use libbreenix::atomic_file as save_barrier;
use canvas::{Canvas,History};
FILE_MODULE
struct App {file_io:file_io::State,width:usize,height:usize,history:History,clean:[u8;32],file:Option<PathBuf>,message:String}
impl App {fn canvas(&self)->&Canvas {self.history.canvas()} METHODS}
DRAIN
#[test] fn large_document() {
 let dir=std::env::temp_dir().join(format!("guskit-open-{}",std::process::id()));std::fs::create_dir(&dir).unwrap();
 for name in ["large.png","large.bmp"] {
  let path=dir.join(name);let mut image=Canvas::new(12,9);image.pencil(&[(11,8)],[17,42,201]);
  let original=files::encode(name,&image,12,9).unwrap();std::fs::write(&path,original).unwrap();
  let small=Canvas::new(8,6);let mut app=App{file_io:file_io::State::default(),width:8,height:6,clean:small.digest(),history:History::new(small),file:None,message:String::new()};
  app.open_file(path.clone());finish_file(&mut app);
  assert_eq!(app.canvas().pixels(),image.pixels(),"opening cropped the drawing");
  assert_eq!((app.width,app.height),(12,9));
  app.save_to(path.clone());finish_file(&mut app);assert_eq!(app.message,"Saved");
  let bytes=std::fs::read(path).unwrap();let (again,_)=files::decode(name,&bytes,12,9).unwrap();
  assert_eq!(again.pixels(),image.pixels(),"resaving lost off-screen pixels");
 }
 std::fs::remove_dir_all(dir).unwrap();
}
}
'''.replace('CANVAS',str(ROOT/'userspace/programs/src/guskit/canvas.rs')).replace('FILES',str(ROOT/'userspace/programs/src/guskit/files.rs')).replace('ATOMIC',str(ROOT/'libs/libbreenix/src/atomic_file.rs')).replace('METHODS',methods).replace('FILE_MODULE',file_module()).replace('DRAIN',DRAIN))
if __name__=='__main__':unittest.main()
