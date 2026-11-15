"""Bound document imports and undo memory without discarding the active drawing."""
import unittest
from guskit_new_save_test import ROOT, run_host, file_methods, file_module, DRAIN

class Memory(unittest.TestCase):
    def test_import_and_history_budget(self):
        methods=file_methods((ROOT/'userspace/programs/src/guskit.rs').read_text(), ['open_file'])
        run_host(self, r'''
#[cfg(test)] mod check {
#[allow(dead_code)] #[path="CANVAS"] mod canvas;
#[allow(dead_code)] #[path="FILES"] mod files;
use canvas::{Canvas,History};
use std::path::PathBuf;
mod libbreenix {#[allow(dead_code)] #[path="ATOMIC"] pub mod atomic_file;}
use libbreenix::atomic_file as save_barrier;
FILE_MODULE
struct App {file_io:file_io::State,width:usize,height:usize,history:History,clean:[u8;32],file:Option<PathBuf>,message:String}
impl App {fn canvas(&self)->&Canvas {self.history.canvas()} METHODS}
DRAIN
#[test] fn rejected_photo_preserves_drawing_and_shows_reason() {
 let path=std::env::temp_dir().join(format!("guskit-photo-{}.bmp",std::process::id()));
 let photo=Canvas::new(4000,3000);std::fs::write(&path,files::encode("photo.bmp",&photo,4000,3000).unwrap()).unwrap();drop(photo);
 let base=Canvas::new(8,6);let clean=base.digest();let mut history=History::new(base);let mut drawn=history.canvas().clone();drawn.pencil(&[(1,1)],[1,2,3]);history.push(drawn);
 let pixels=history.canvas().pixels().to_vec();
 let mut app=App {file_io:file_io::State::default(),width:8,height:6,history,clean,file:Some(PathBuf::from("drawing.png")),message:String::new()};
 app.open_file(path.clone());finish_file(&mut app);
 assert!(app.message.contains("document memory limit"),"{}",app.message);
 assert_eq!(app.canvas().pixels(),pixels);assert_eq!((app.width,app.height),(8,6));assert_eq!(app.file,Some(PathBuf::from("drawing.png")));assert_ne!(app.canvas().digest(),app.clean);assert_eq!(app.history.undo_depth(),1);
 assert!(app.history.undo());assert_eq!(app.canvas().digest(),clean);
 std::fs::remove_file(path).unwrap();
}
#[test] fn oversized_import_is_refused_before_decode() {
 // CORE headers store unsigned 16-bit dimensions rather than INFO's 32-bit fields.
 let mut core=vec![0u8;30];core[..2].copy_from_slice(b"BM");core[2..6].copy_from_slice(&30u32.to_le_bytes());core[10..14].copy_from_slice(&26u32.to_le_bytes());core[14..18].copy_from_slice(&12u32.to_le_bytes());
 core[18..20].copy_from_slice(&1u16.to_le_bytes());core[20..22].copy_from_slice(&1u16.to_le_bytes());core[22..24].copy_from_slice(&1u16.to_le_bytes());core[24..26].copy_from_slice(&24u16.to_le_bytes());core[26..29].copy_from_slice(&[3,2,1]);
 let (small,message)=files::decode("small.bmp",&core,0,0).expect("fitting CORE bitmap refused");assert_eq!(small.pixels(),[1,2,3]);assert!(message.is_none());
 core[18..20].copy_from_slice(&4000u16.to_le_bytes());core[20..22].copy_from_slice(&3000u16.to_le_bytes());
 assert!(files::decode("large.bmp",&core,0,0).err().unwrap().contains("document memory limit"));
 let mut bmp=vec![0u8;54];bmp[..2].copy_from_slice(b"BM");bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
 bmp[18..22].copy_from_slice(&4000i32.to_le_bytes());bmp[22..26].copy_from_slice(&3000i32.to_le_bytes());
 let err=files::decode("photo.bmp",&bmp,0,0).err().expect("oversized BMP accepted");
 assert!(err.contains("document memory limit"),"{err}");
 let mut png=vec![0u8;33];png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");png[8..12].copy_from_slice(&13u32.to_be_bytes());png[12..16].copy_from_slice(b"IHDR");
 png[16..20].copy_from_slice(&4000u32.to_be_bytes());png[20..24].copy_from_slice(&3000u32.to_be_bytes());
 let err=files::decode("photo.png",&png,0,0).err().expect("oversized PNG accepted");
 assert!(err.contains("document memory limit"),"{err}");
}
#[test] fn edits_and_growth_keep_bounded_undo() {
 let mut history=History::new(Canvas::new(1024,1024));
 for n in 0..14 {let mut c=history.canvas().clone();c.pencil(&[(0,0)],[n,2,3]);history.push(c);}
 let bytes=1024*1024*3;
 assert!((history.undo_depth()+1)*bytes<=32*1024*1024,"undo exceeds memory budget");
 assert!(history.undo());let redo=history.canvas().pixels()[..3].to_vec();assert!(history.redo());
 history.grow(2048,1024);
 assert!((history.undo_depth()+history.redo_depth()+1)*bytes*2<=32*1024*1024,"growth exceeds memory budget");
 assert_eq!(&history.canvas().pixels()[..3],&[13,2,3]);assert!(history.undo());
 assert_eq!(&history.canvas().pixels()[..3],redo);assert!(history.redo());
}
}
'''.replace('CANVAS',str(ROOT/'userspace/programs/src/guskit/canvas.rs')).replace('FILES',str(ROOT/'userspace/programs/src/guskit/files.rs')).replace('ATOMIC',str(ROOT/'libs/libbreenix/src/atomic_file.rs')).replace('FILE_MODULE',file_module()).replace('METHODS',methods).replace('DRAIN',DRAIN))

if __name__=='__main__':unittest.main()
