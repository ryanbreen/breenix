//! Run the controlled app's production worker/command path with host render boundaries.
extern crate self as breengel;
extern crate self as libbreenix;
extern crate self as libgfx;
extern crate self as libuitest;
#[path = "../libs/libuitest/src/capture_marker.rs"] pub mod capture_marker;
#[path = "../userspace/programs/src/uitest_capture_app.rs"] mod app;
#[path = "../userspace/programs/src/uitest_app_support.rs"] mod support;
use std::{sync::{atomic::{AtomicU32, AtomicU64, Ordering}, Mutex}, io::{BufRead, BufReader, Write}, os::unix::net::UnixListener};
static NEXT_TID: AtomicU32 = AtomicU32::new(1);
thread_local! { static TID: u32 = NEXT_TID.fetch_add(1, Ordering::Relaxed); }
pub mod process {
    pub struct Tid(u32);
    impl Tid { pub fn raw(&self) -> u64 { self.0.into() } }
    pub fn gettid() -> Result<Tid, String> { Ok(Tid(crate::TID.with(|tid| *tid))) }
}
pub mod color {
    pub struct Color(pub [u8; 4]);
    impl Color { pub fn rgb(r:u8,g:u8,b:u8) -> Self { Self([b,g,r,255]) } }
}
pub struct Surface { pixels: Vec<u8>, width: usize }
impl Surface {
    pub fn batch(&mut self, f: impl FnOnce(&mut Self)) { f(self); }
    pub fn clear(&mut self, color: color::Color) { for pixel in self.pixels.chunks_exact_mut(4) { pixel.copy_from_slice(&color.0); } }
}
pub mod shapes {
    pub fn fill_rect(surface:&mut crate::Surface, x:i32,y:i32,w:i32,h:i32,color:crate::color::Color) {
        for row in y..y+h { for col in x..x+w {
            let at=(row as usize*surface.width+col as usize)*4;
            surface.pixels[at..at+4].copy_from_slice(&color.0);
        } }
    }
}
pub mod capture {
    pub fn root(g:crate::capture_marker::Generation, _:u32, _:u32)->Vec<u8> { format!("{}/{}",g.commit,g.producer).into_bytes() }
}
#[derive(Clone,Debug)] struct Snapshot { id:u64, width:u32, height:u32, generation:capture_marker::Generation }
static SNAPSHOTS:Mutex<Vec<Snapshot>>=Mutex::new(Vec::new());
static LIVE:Mutex<Vec<u64>>=Mutex::new(Vec::new());
static NEXT_ID:AtomicU64=AtomicU64::new(1);
pub mod window {
    pub mod abi { pub struct Rect; impl Rect { pub fn new(_:i32,_:i32,_:i32,_:i32)->Self { Self } } }
    pub fn destroy(id:u64)->Result<u64,String> {
        let mut live=crate::LIVE.lock().unwrap();
        if !live.contains(&id) {return Err("ENOENT".into());}
        live.retain(|&seen|seen!=id); Ok(0)
    }
}
pub struct Window { id:u64,width:u32,height:u32,commit:u64,tree:Vec<u8>,surface:Surface }
pub struct Committed {pub commit_id:u64}
impl Window {
    pub fn new_without_fonts(_: &[u8],width:u32,height:u32)->Result<Self,String> {
        let id=NEXT_ID.fetch_add(1,Ordering::Relaxed); LIVE.lock().unwrap().push(id);
        Ok(Self{id,width,height,commit:0,tree:Vec::new(),surface:Surface{pixels:vec![0;width as usize*height as usize*4],width:width as usize}})
    }
    pub fn id(&self)->u64 {self.id}
    pub fn width(&self)->u32 {self.width}
    pub fn height(&self)->u32 {self.height}
    pub fn acquire(&mut self, timeout:u64)->Result<(),String> {assert!(timeout>0 && timeout<1_000_000_000);Ok(())}
    pub fn apply_resize(&mut self,width:u32,height:u32)->Result<(),String> {
        self.width=width;self.height=height;self.surface=Surface{pixels:vec![0;width as usize*height as usize*4],width:width as usize};Ok(())
    }
    pub fn surface(&mut self)->Result<&mut Surface,String> {Ok(&mut self.surface)}
    pub fn write_tree(&mut self, bytes:&[u8])->Result<(),String> {self.tree=bytes.to_vec();Ok(())}
    pub fn commit(&mut self,_:&[window::abi::Rect])->Result<Committed,String> {
        self.commit+=1;
        let g=capture_marker::decode(&self.surface.pixels[..32]).unwrap();
        assert_eq!(g.commit,self.commit);
        assert_eq!(g.producer,TID.with(|tid|*tid));
        assert_eq!(self.tree,format!("{}/{}",self.commit,g.producer).as_bytes());
        assert_eq!(self.surface.pixels,capture_marker::frame(self.width as usize,self.height as usize,g).unwrap());
        SNAPSHOTS.lock().unwrap().push(Snapshot{id:self.id,width:self.width,height:self.height,generation:g});
        Ok(Committed{commit_id:self.commit})
    }
}
impl Drop for Window {fn drop(&mut self) {let _=window::destroy(self.id);}}

#[test]
fn app_commands_alternate_real_workers_and_keep_tree_rows_and_ordinal() {
    let root=std::env::temp_dir().join(format!("ui-capture-app-{}",std::process::id()));
    std::fs::create_dir(&root).unwrap();
    std::env::set_var("BREENIX_CONFIG_ROOT",&root);
    let listener=UnixListener::bind(root.join("capture-control.sock")).unwrap();
    let worker=std::thread::spawn(||app::serve("capture test"));
    let (socket,_)=listener.accept().unwrap();
    socket.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
    let mut socket=BufReader::new(socket);
    let mut command=|command:&str|->Vec<u64> {
        writeln!(socket.get_mut(),"{command}").unwrap();
        let mut line=String::new();socket.read_line(&mut line).unwrap();
        let mut words=line.split_whitespace();assert_eq!(words.next(),Some("ok"));
        words.map(|s|s.parse().unwrap()).collect()
    };
    let a=command("create"); let b=command("commit 1");let c=command("commit 0");
    assert_eq!(a[1],1);assert_eq!(b[1],2);assert_eq!(c[1],3);
    assert_ne!(a[2],b[2]);assert_eq!(a[2],c[2]);assert_ne!(a[2],0);
    let resized=command("resize");assert_eq!(resized,vec![a[0],4,a[2],160,100]);
    assert_eq!(command("destroy"),resized); assert!(LIVE.lock().unwrap().is_empty());
    let recreated=command("create");assert_ne!(a[0],recreated[0]);assert_eq!(recreated[1],1);
    assert_eq!(command("quit"),vec![0;5]);assert_eq!(worker.join().unwrap(),Ok(()));
    assert!(LIVE.lock().unwrap().is_empty());
    let snapshots=SNAPSHOTS.lock().unwrap();assert_eq!(snapshots.len(),5);
    assert_eq!((snapshots[0].id,snapshots[0].width,snapshots[0].height,snapshots[0].generation.commit),(a[0],320,200,1));
    assert_eq!(snapshots[3].generation.producer,a[2] as u32);
    std::env::remove_var("BREENIX_CONFIG_ROOT");std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn capture_control_is_explicit_and_exclusive() {
    assert!(!support::parse(&["window"]).unwrap().capture_control);
    let options=support::parse(&["window","--capture-control"]).unwrap();
    assert!(options.capture_control);assert_eq!(options.title,"window");
    assert_eq!((options.crash,options.hang,options.stderr_flood,options.session_probe,options.config_probe,options.canonical),(None,false,0,false,false,false));
    for flag in ["--canonical","--hang","--crash=11","--session-probe","--config-probe","--stderr-flood=0","--capture-control"] {
        assert!(support::parse(&["window","--capture-control",flag]).is_err(),"{flag}");
    }
    assert!(support::echo(&[b"app"],&[]).starts_with("UITEST-ECHO 1 "));
}
