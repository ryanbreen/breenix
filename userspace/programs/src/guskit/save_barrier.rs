//! Deterministic interruption of an app save in a suite-mode boot.
use libbreenix::atomic_file::{self, Operations, SaveResult, StdOperations};
use std::{fs, io, path::{Path, PathBuf}};

#[path = "save_wait.rs"] mod save_wait;
use save_wait::wait;

const REQUEST: &str = "/run/breenix/guskit-save-barrier";

pub fn save(path: &Path, bytes: &[u8]) -> SaveResult {
    if libbreenix::test_control::state().is_ok_and(|state| state == 1) {
        if let Ok(request) = fs::read_to_string(REQUEST) {
            if let Some((dest, control)) = request.trim_end().split_once('\n') {
                if Path::new(dest) == path {
                    return atomic_file::save_with(&mut Barrier {
                        inner: StdOperations, control: control.into(), partial: false,
                    }, path, bytes);
                }
            }
        }
    }
    atomic_file::save(path, bytes)
}


struct Barrier<O> { inner: O, control: PathBuf, partial: bool }
impl<O: Operations> Operations for Barrier<O> {
    type File = O::File;
    fn create_temp(&mut self, dest: &Path) -> io::Result<(PathBuf, Self::File)> {
        self.inner.create_temp(dest)
    }
    fn write(&mut self, file: &mut Self::File, bytes: &[u8]) -> io::Result<usize> {
        if !self.partial {
            let n = self.inner.write(file, &bytes[..bytes.len().min(16)])?;
            if n == 0 { return Ok(0); }
            self.inner.sync_file(file)?;
            fs::write(self.control.join("partial"), n.to_string())?;
            self.partial = true;
            wait(&self.control, "stopped")?;
            return Ok(n);
        }
        match self.inner.write(file, bytes) {
            Err(e) => {
                fs::write(self.control.join("failed"), e.raw_os_error().unwrap_or(0).to_string())?;
                // Restore transport before atomic_file performs its normal temp cleanup.
                wait(&self.control, "recovered")?;
                Err(e)
            }
            result => result,
        }
    }
    fn sync_file(&mut self, file: &Self::File) -> io::Result<()> { self.inner.sync_file(file) }
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> { self.inner.rename(from, to) }
    fn sync_dir(&mut self, dir: &Path) -> io::Result<()> { self.inner.sync_dir(dir) }
    fn remove_temp(&mut self, path: &Path) -> io::Result<()> { self.inner.remove_temp(path) }
}
