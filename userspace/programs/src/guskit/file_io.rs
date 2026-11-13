//! File work starts only after the requesting input's busy frame commits.
use super::{canvas::Canvas, files, save_barrier};
use std::{path::PathBuf, thread::JoinHandle};

pub enum Request {
    Save { path: Option<PathBuf>, image: Canvas, width: usize, height: usize },
    SaveTo { path: PathBuf, image: Canvas, width: usize, height: usize },
    Open(PathBuf),
}
pub enum Completion {
    Saved { path: PathBuf, clean: [u8; 32] },
    Opened { path: PathBuf, image: Canvas, message: String },
}
impl Request {
    fn status(&self) -> &'static str { if matches!(self, Self::Open(_)) { "Loading" } else { "Saving" } }
    pub fn run(self) -> Result<Completion, String> {
        match self {
            Self::Open(path) => {
                let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                let (image, message) = files::decode(&path.to_string_lossy(), &bytes, 0, 0)?;
                Ok(Completion::Opened { path, image, message: message.unwrap_or_default() })
            }
            Self::SaveTo { path, image, width, height } => {
                let bytes = files::encode(&path.to_string_lossy(), &image, width, height)?;
                match save_barrier::save(&path, &bytes) {
                    libbreenix::atomic_file::SaveResult::Saved => Ok(Completion::Saved { path, clean: image.digest() }),
                    libbreenix::atomic_file::SaveResult::Failed(e) | libbreenix::atomic_file::SaveResult::Unconfirmed(e) => Err(e.to_string()),
                }
            }
            Self::Save { path: Some(path), image, width, height } => {
                let bytes = files::encode(&path.to_string_lossy(), &image, width, height)?;
                match save_barrier::save(&path, &bytes) {
                    libbreenix::atomic_file::SaveResult::Saved => Ok(Completion::Saved { path, clean: image.digest() }),
                    libbreenix::atomic_file::SaveResult::Failed(e) | libbreenix::atomic_file::SaveResult::Unconfirmed(e) => Err(e.to_string()),
                }
            }
            Self::Save { path: None, image, width, height } => {
                // Reserve the first free name atomically, retaining existing drawings.
                use std::io::Write;
                let bytes = files::encode("untitled.png", &image, width, height)?;
                let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
                for n in 1u64.. {
                    let path = home.join(format!("untitled-{n}.png"));
                    let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                        Ok(file) => file,
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                        Err(e) => return Err(e.to_string()),
                    };
                    let result = file.write_all(&bytes).and_then(|()| file.sync_all());
                    drop(file);
                    if let Err(e) = result { let _ = std::fs::remove_file(&path); return Err(e.to_string()); }
                    std::fs::File::open(&home).and_then(|dir| dir.sync_all()).map_err(|e| e.to_string())?;
                    return Ok(Completion::Saved { path, clean: image.digest() });
                }
                Err("no free untitled filename".into())
            }
        }
    }
}

#[derive(Default)]
pub enum State {
    #[default] Idle,
    Pending(Request),
    Running(&'static str, JoinHandle<Result<Completion, String>>),
}
impl State {
    pub fn status(&self) -> Option<&'static str> {
        match self { Self::Idle => None, Self::Pending(request) => Some(request.status()), Self::Running(status, _) => Some(status) }
    }
    pub fn busy(&self) -> bool { !matches!(self, Self::Idle) }
    pub fn queue(&mut self, request: Request) -> Result<&'static str, String> {
        if let Some(status) = self.status() { return Err(format!("{status}: file operation already in progress")); }
        let status = request.status();
        *self = Self::Pending(request);
        Ok(status)
    }
    pub fn start(&mut self) -> Result<(), String> {
        if !matches!(self, Self::Pending(_)) { return Ok(()); }
        let Self::Pending(request) = std::mem::take(self) else { unreachable!() };
        let status = request.status();
        let worker = std::thread::Builder::new().name("guskit-file".into()).spawn(move || request.run())
            .map_err(|e| e.to_string())?;
        *self = Self::Running(status, worker);
        Ok(())
    }
    pub fn poll(&mut self) -> Option<Result<Completion, String>> {
        if !matches!(self, Self::Running(_, worker) if worker.is_finished()) { return None; }
        let Self::Running(_, worker) = std::mem::take(self) else { unreachable!() };
        Some(worker.join().unwrap_or_else(|_| Err("File worker failed".into())))
    }
}
