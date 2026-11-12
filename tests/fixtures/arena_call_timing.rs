#[path = "arena.rs"]
pub mod arena;
use std::sync::{Mutex, atomic::{AtomicBool, AtomicU64, Ordering::Relaxed}};
static ACTIVE: AtomicBool = AtomicBool::new(false);
static CPU_SUPPORTED: AtomicBool = AtomicBool::new(true);
static WALL: AtomicU64 = AtomicU64::new(0);
static CPU: AtomicU64 = AtomicU64::new(0);
static OPS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static OUTPUT: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
fn tick(name: &str) {
    if ACTIVE.load(Relaxed) {
        let mut ops = OPS.lock().unwrap();
        ops.push(name.into());
        WALL.fetch_add(ops.len() as u64 * 100, Relaxed);
        CPU.fetch_add(ops.len() as u64 * 7, Relaxed);
    }
}
fn reset() { OPS.lock().unwrap().clear(); ACTIVE.store(true, Relaxed); }
pub struct Clock(u64);
impl Clock {
    pub fn now() -> Self { Self(WALL.load(Relaxed)) }
    pub fn elapsed(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(WALL.load(Relaxed) - self.0)
    }
}
#[no_mangle]
unsafe extern "C" fn fixture_clock_gettime(id: i32, ts: *mut i64) -> i32 {
    assert_eq!(id, if cfg!(target_os="macos") {16} else {3});
    if !CPU_SUPPORTED.load(Relaxed) { return -1; }
    let ns = CPU.load(Relaxed);
    ts.write((ns / 1_000_000_000) as i64);
    ts.add(1).write((ns % 1_000_000_000) as i64);
    0
}
extern "C" {
    #[link_name="mmap"] fn host_mmap(a:*mut u8,l:usize,p:i32,f:i32,fd:i32,o:i64)->*mut u8;
    #[link_name="munmap"] fn host_munmap(a:*mut u8,l:usize)->i32;
}
#[no_mangle]
unsafe extern "C" fn system_mmap(a:*mut u8,l:usize,p:i32,f:i32,fd:i32,o:i64)->*mut u8 {
    tick("mmap"); host_mmap(a,l,p,f,fd,o)
}
#[no_mangle]
unsafe extern "C" fn system_munmap(a:*mut u8,l:usize)->i32 {
    tick("munmap"); host_munmap(a,l)
}
pub mod fixture_fs {
    pub use std::fs::{symlink_metadata, remove_file};
    use std::{fs, io::{self, Read, Write}, path::Path, os::fd::AsRawFd};
    pub struct File(Option<fs::File>);
    impl std::ops::Deref for File {
        type Target = fs::File;
        fn deref(&self)->&Self::Target { self.0.as_ref().unwrap() }
    }
    impl File {
        pub fn set_len(&self, len:u64)->io::Result<()> {
            super::tick("set_len"); self.deref_file().set_len(len)
        }
        fn deref_file(&self)->&fs::File { self.0.as_ref().unwrap() }
    }
    impl AsRawFd for File { fn as_raw_fd(&self)->i32 { self.deref_file().as_raw_fd() } }
    impl Drop for File { fn drop(&mut self) { if self.0.is_some() { super::tick("close"); } } }
    impl Read for File {
        fn read(&mut self, b:&mut [u8])->io::Result<usize> { self.0.as_mut().unwrap().read(b) }
    }
    impl Write for File {
        fn write(&mut self, b:&[u8])->io::Result<usize> {
            if let Some(file) = self.0.as_mut() {
                super::tick("header_write"); file.write(b)
            } else {
                super::OUTPUT.lock().unwrap().push(b.to_vec()); Ok(b.len())
            }
        }
        fn flush(&mut self)->io::Result<()> { Ok(()) }
    }
    pub struct OpenOptions(fs::OpenOptions);
    impl Default for OpenOptions { fn default()->Self { Self::new() } }
    impl OpenOptions {
        pub fn new()->Self { Self(fs::OpenOptions::new()) }
        pub fn read(&mut self,v:bool)->&mut Self { self.0.read(v); self }
        pub fn write(&mut self,v:bool)->&mut Self { self.0.write(v); self }
        pub fn create_new(&mut self,v:bool)->&mut Self { self.0.create_new(v); self }
        pub fn open(&self,p:impl AsRef<Path>)->io::Result<File> {
            if p.as_ref()==Path::new("/dev/screen-evidence") { return Ok(File(None)); }
            super::tick("open"); self.0.open(p).map(|f| File(Some(f)))
        }
    }
    impl std::os::unix::fs::OpenOptionsExt for OpenOptions {
        fn mode(&mut self,m:u32)->&mut Self { self.0.mode(m); self }
        fn custom_flags(&mut self,f:i32)->&mut Self { self.0.custom_flags(f); self }
    }
    pub fn rename(a:impl AsRef<Path>,b:impl AsRef<Path>)->io::Result<()> {
        super::tick("rename"); fs::rename(a,b)
    }
}
fn check_record(index: usize, names: &[&str], cpu: bool, ok: bool) {
    let output = OUTPUT.lock().unwrap();
    assert_eq!(output.len(), index+1, "one immediate record per completed checkpoint");
    let bytes = &output[index];
    assert!(bytes.len() <= 1024 && bytes.ends_with(b"\n"), "bounded complete VALUE record");
    let line = std::str::from_utf8(bytes).unwrap();
    let mut fields = line.split_whitespace();
    assert_eq!(fields.next(),Some("VALUE"));
    assert_eq!(fields.next(),Some("arena-call-ns"));
    assert_eq!(fields.next(),Some(if ok {"ok"} else {"error"}));
    let values:Vec<_> = fields.collect();
    assert_eq!(values.len(),14, "fixed call table including absent payloads");
    let order = ["open", "set_len", "header_write", "mmap_screen", "mmap_frame", "mmap_tree",
                 "rename", "close", "munmap_header_screen", "munmap_header_frame", "munmap_header_tree",
                 "munmap_payload_screen", "munmap_payload_frame", "munmap_payload_tree"];
    for (name,field) in order.iter().zip(values) {
        let (label,value)=field.split_once('=').unwrap(); assert_eq!(label,*name);
        if let Some(n) = names.iter().position(|n| n==name) {
            let cpu_value = if cpu { ((n+1)*7).to_string() } else { "unsupported".into() };
            assert_eq!(value,format!("{}/{}",(n+1)*100,cpu_value), "wrong interval for {name}");
        } else { assert_eq!(value,"-", "absent call {name} must not invent time"); }
    }
}
fn main() {
    use arena::{Arena, Layout, Slot, SlotRead};
    let root=std::env::temp_dir().join(format!("arena-calls-{}",std::process::id()));
    std::fs::create_dir(&root).unwrap(); let path=root.join("arena");
    let mut arena=Arena::create(&path,Layout::for_mode(8,4).unwrap()).unwrap();
    for slot in [Slot::Screen,Slot::Frame,Slot::Tree] { drop(arena.begin(slot).unwrap()); }
    reset();
    arena.next_checkpoint().unwrap();
    let full = ["open", "set_len", "header_write", "mmap_screen", "mmap_frame", "mmap_tree",
                "rename", "close", "munmap_header_screen", "munmap_header_frame", "munmap_header_tree",
                "munmap_payload_screen", "munmap_payload_frame", "munmap_payload_tree"];
    check_record(0,&full,true,true);
    assert_eq!(*OPS.lock().unwrap(),["open","set_len","header_write","mmap","mmap","mmap",
                                   "rename","close","munmap","munmap","munmap","munmap","munmap","munmap"]);
    assert!(matches!(arena.read(Slot::Frame).unwrap(),SlotRead::Free));
    CPU_SUPPORTED.store(false,Relaxed); reset();
    arena.next_checkpoint().unwrap();
    check_record(1,&full[..11],false,true);
    // The existing create_new refusal must retain the current arena and its path.
    std::fs::write(path.with_extension("next"),b"occupied").unwrap(); reset();
    assert!(arena.next_checkpoint().is_err());
    check_record(2,&full[..1],false,false);
    assert_eq!(*OPS.lock().unwrap(),["open"]);
    assert!(matches!(arena.read(Slot::Frame).unwrap(),SlotRead::Free));
    ACTIVE.store(false,Relaxed); drop(arena);
    std::fs::remove_dir_all(root).unwrap();
    println!("checkpoint call timing, CPU fallback, drop order and create_new refusal PASS");
}
