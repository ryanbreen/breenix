#[cfg(test)] mod check {
use std::{path::{Path,PathBuf}, sync::{Mutex,Condvar}};
#[allow(dead_code)] #[path="CANVAS_PATH"] mod canvas;
#[path="FILES_PATH"] mod files;
mod libbreenix {
    #[allow(dead_code)] #[path="ATOMIC_PATH"] pub mod atomic_file;
    pub mod input {pub mod abi {pub fn advance_watermark(_:u64,serial:u64)->u64 {serial}}}
    pub mod window {pub mod abi {pub type Rect=crate::check::Clip;}}
}
use canvas::{Canvas,History};
#[derive(Default)] struct Observation {commits:Vec<(u64,String)>,entered:bool,release:bool}
static OBS: Mutex<Observation> = Mutex::new(Observation{commits:Vec::new(),entered:false,release:false});
static TEST: Mutex<()> = Mutex::new(());
static WAKE: Condvar = Condvar::new();
fn block_io() {
    let mut obs=OBS.lock().unwrap();obs.entered=true;WAKE.notify_all();
    while !obs.release {obs=WAKE.wait(obs).unwrap();}
}
fn blocked_read(path:&Path)->std::io::Result<Vec<u8>> {block_io();std::fs::read(path)}
mod save_barrier {
    pub fn save(path:&std::path::Path,bytes:&[u8])->super::libbreenix::atomic_file::SaveResult {
        super::block_io();super::libbreenix::atomic_file::save(path,bytes)
    }
}
// FILE_IO_MODULE
struct App {
    width:usize,height:usize,history:History,clean:[u8;32],file:Option<PathBuf>,message:String,damage:Option<Clip>,
    // FILE_IO_FIELD
}
impl App {
    fn canvas(&self)->&Canvas {self.history.canvas()}
    fn dirty(&self)->bool {self.canvas().digest()!=self.clean}
    // FILE_METHODS
}
#[derive(Clone,Copy)] pub struct Clip {x:i32,y:i32,w:i32,h:i32}
impl Clip {fn new(x:i32,y:i32,w:i32,h:i32)->Self {Self{x,y,w,h}} fn is_empty(&self)->bool {self.w<=0||self.h<=0}}
struct MenuBar;struct CachedFont;
#[derive(Debug)] struct Error;
impl std::fmt::Display for Error {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {f.write_str("refused")}}
#[derive(PartialEq)] enum Handled {Redraw}
#[allow(dead_code)] struct Delivered {serial:u64,time_ns:u64,clicks:u8,event:Event}
impl Delivered {fn from_raw(r:&abi::WindowEvent)->Self {Self{serial:r.serial,time_ns:0,clicks:0,event:Event::FontChanged}}}
mod abi {pub struct WindowEvent {pub serial:u64}}
struct FontWatcher;
impl FontWatcher {fn poll(&self)->Result<Option<()>,()> {Ok(None)}}
// The production deliver method also has a font event literal.
struct Event;
#[allow(non_upper_case_globals)] impl Event {const FontChanged:Self=Self;}
struct Window {handled:u64,since_pixel:Vec<(u64,Handled)>,font_watcher:FontWatcher,held:u64,tree:String,fail:bool}
struct Acquired {replay_full:bool,replay:Vec<Clip>}
impl Window {
    fn deliver_clock(&mut self,_:&mut dyn FnMut(&Delivered)->Handled)->usize {0}
    // DELIVER
    fn width(&self)->u32 {80} fn height(&self)->u32 {60}
    fn acquire(&mut self,_:u64)->Result<Acquired,Error> {
        if self.fail {return Err(Error)} self.held=self.handled;Ok(Acquired{replay_full:false,replay:vec![]})
    }
    fn surface(&mut self)->Result<&mut Self,Error> {Ok(self)}
    fn commit(&mut self,_:&[Clip])->Result<(),Error> {
        OBS.lock().unwrap().commits.push((self.held,self.tree.clone()));Ok(())
    }
}
fn render(win:&mut Window,app:&mut App,_:&MenuBar,_:&mut Option<CachedFont>,_:f32)->Result<(),String> {
    win.tree=app.message.clone();Ok(())
}
fn publish(win:&mut Window,app:&App,_:&MenuBar,_:f32) {assert_eq!(win.tree,app.message)}
// PRESENT_FRAME
fn frame(win:&mut Window,app:&mut App) {
    let menu=MenuBar;let mut font=None;let font_size=14.0;let redraw=true;
    // FRAME
    assert!(!presentation_failed);
}
fn complete(app:&mut App) {
    assert!(app.width>0);
    // POLL
    // BUSY_CHECK
}
fn scenario(open:bool,save_as:bool) {
    let _guard=TEST.lock().unwrap_or_else(|e|e.into_inner());
    *OBS.lock().unwrap()=Observation::default();
    let dir=std::env::temp_dir().join(format!("guskit-async-{}-{open}-{save_as}",std::process::id()));std::fs::create_dir(&dir).unwrap();
    let path=dir.join("drawing.png");let mut image=Canvas::new(8,6);image.pencil(&[(7,5)],[12,34,56]);
    let encoded=files::encode("drawing.png",&image,8,6).unwrap();std::fs::write(&path,&encoded).unwrap();
    let expected=image.digest();
    let ui=std::thread::spawn(move || {
        let blank=Canvas::new(8,6);
        let mut app=App{width:8,height:6,clean:blank.digest(),history:History::new(if open {blank} else {image}),file:Some(path.clone()),message:String::new(),damage:None,
            // FILE_IO_INIT
        };
        let mut win=Window{handled:0,since_pixel:vec![],font_watcher:FontWatcher,held:0,tree:String::new(),fail:false};
        win.deliver(&[abi::WindowEvent{serial:41}],&mut |_| {
            if open {app.open_file(path.clone())} else if save_as {app.save_to(path.clone())} else {app.save()}
            Handled::Redraw
        });
        // A refused presentation must not let pending I/O escape.
        win.fail=true;assert!(present_frame(&mut win,&mut app,&MenuBar,&mut None,14.0).is_err());
        assert!(!OBS.lock().unwrap().entered);win.fail=false;
        frame(&mut win,&mut app);
        // A second save during I/O must be refused visibly, not replace the job.
        if !open {app.save();assert!(app.message.contains("progress"),"second save was silently lost: {}",app.message);}
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
        loop {assert!(std::time::Instant::now()<deadline,"file completion did not reach the UI");complete(&mut app);if app.message!="Saving"&&app.message!="Loading"&&!app.message.contains("progress") {break} std::thread::yield_now();}
        assert!(!app.dirty());assert_eq!(app.canvas().digest(),expected);
        if !open {assert_eq!(app.message,"Saved");assert_eq!(std::fs::read(&path).unwrap(),encoded)}
        else {assert!(app.message.is_empty())}
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    });
    let mut obs=OBS.lock().unwrap();
    let (next,timeout)=WAKE.wait_timeout_while(obs,std::time::Duration::from_secs(10),|s|!s.entered).unwrap();
    obs=next;assert!(!timeout.timed_out(),"file operation never reached its I/O boundary");
    let committed=obs.commits.clone();obs.release=true;WAKE.notify_all();drop(obs);
    let joined=ui.join();
    assert!(committed.iter().any(|(serial,status)|*serial==41&&status==if open {"Loading"}else{"Saving"}),
        "file I/O started before the requesting input's busy frame committed: {committed:?}");
    joined.unwrap();
}
#[test] fn blocked_save() {scenario(false,false)}
#[test] fn blocked_save_as() {scenario(false,true)}
#[test] fn blocked_open() {scenario(true,false)}
}
