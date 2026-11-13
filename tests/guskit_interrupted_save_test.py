"""Interrupt the app's production save adapter after an actual partial write."""
from pathlib import Path
import os
import subprocess
import unittest
from suite_collection_test import run_rust
ROOT = Path(__file__).resolve().parents[1]

class InterruptedSave(unittest.TestCase):
    def test_partial_write_then_disconnect_preserves_destination_and_cleans_temp(self):
        rel = 'userspace/programs/src/guskit.rs'
        old = os.environ.get('GUSKIT_OLD_COMMIT')
        app = subprocess.check_output(['git', 'show', f'{old}:{rel}'], text=True) if old else (ROOT / rel).read_text()
        if 'mod file_io;' in app:
            rel = 'userspace/programs/src/guskit/file_io.rs'
            app = subprocess.check_output(['git', 'show', f'{old}:{rel}'], text=True) if old else (ROOT / rel).read_text()
        self.assertEqual(app.count('save_barrier::save(&path, &bytes)'), 2)
        source = (ROOT / 'userspace/programs/src/guskit/save_barrier.rs').read_text()
        source = source.replace('//! Deterministic interruption of an app save in a suite-mode boot.', '')
        source = source.replace('atomic_file::{self,', 'atomic_file::{').replace('#[path = "save_wait.rs"]', f'#[path = "{ROOT}/userspace/programs/src/guskit/save_wait.rs"]')
        run_rust(self, r'''
extern crate self as libbreenix;
#[path = "ATOMIC"] pub mod atomic_file;
pub mod test_control { pub fn state() -> Result<u64, ()> { Ok(0) } }
SOURCE
struct Remote { inner: StdOperations, connected: std::sync::Arc<std::sync::atomic::AtomicBool> }
impl Operations for Remote {
 type File = fs::File;
 fn create_temp(&mut self, p: &Path) -> io::Result<(PathBuf, Self::File)> { self.inner.create_temp(p) }
 fn write(&mut self, f: &mut Self::File, b: &[u8]) -> io::Result<usize> {
  if !self.connected.load(std::sync::atomic::Ordering::SeqCst) { return Err(io::Error::from_raw_os_error(5)); }
  self.inner.write(f,b)
 }
 fn sync_file(&mut self, f: &Self::File) -> io::Result<()> { self.inner.sync_file(f) }
 fn rename(&mut self, a: &Path, b: &Path) -> io::Result<()> { self.inner.rename(a,b) }
 fn sync_dir(&mut self, p: &Path) -> io::Result<()> { self.inner.sync_dir(p) }
 fn remove_temp(&mut self, p: &Path) -> io::Result<()> {
  assert!(self.connected.load(std::sync::atomic::Ordering::SeqCst)); self.inner.remove_temp(p)
 }
}
#[test] fn interrupted_app_save() {
 use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
 let _app_save: fn(&Path,&[u8])->SaveResult = save;
 let root = std::env::temp_dir().join(format!("guskit-interrupt-{}",std::process::id()));
 fs::create_dir(&root).unwrap(); let control=root.join("control"); fs::create_dir(&control).unwrap();
 let dest=root.join("drawing.png"); fs::write(&dest,b"old drawing").unwrap();
 let connected=Arc::new(AtomicBool::new(true));
 let (peer,c,d,r)=(connected.clone(),control.clone(),dest.clone(),root.clone());
 let server=std::thread::spawn(move || {
  wait(&c,"partial").unwrap();
  assert_eq!(fs::read(&d).unwrap(),b"old drawing");
  let temp=fs::read_dir(&r).unwrap().map(Result::unwrap).find(|e|e.file_name().to_string_lossy().starts_with(".drawing.png.atomic-")).unwrap();
  assert_eq!(fs::read(temp.path()).unwrap(),[b'n';16]);
  peer.store(false,Ordering::SeqCst); fs::write(c.join("stopped"),b"").unwrap();
  wait(&c,"failed").unwrap(); assert_eq!(fs::read(c.join("failed")).unwrap(),b"5");
  peer.store(true,Ordering::SeqCst); fs::write(c.join("recovered"),b"").unwrap();
 });
 let result=atomic_file::save_with(&mut Barrier { inner:Remote { inner:StdOperations,connected },control,partial:false }, &dest, &[b'n';128]);
 assert!(matches!(result,SaveResult::Failed(e) if e.raw_os_error()==Some(5)));
 server.join().unwrap(); assert_eq!(fs::read(dest).unwrap(),b"old drawing");
 assert_eq!(fs::read_dir(&root).unwrap().count(),2); fs::remove_dir_all(root).unwrap();
}
'''.replace('ATOMIC', str(ROOT / 'libs/libbreenix/src/atomic_file.rs')).replace('SOURCE', source))

if __name__ == '__main__': unittest.main()
