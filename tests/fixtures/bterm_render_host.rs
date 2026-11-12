// Production VT, terminal paint and TTF batching; the device records sample rectangles.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::alloc::{GlobalAlloc, Layout, System};
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
struct CountAlloc;
unsafe impl GlobalAlloc for CountAlloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { ALLOCS.fetch_add(1, Ordering::Relaxed); System.alloc(l) }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { System.dealloc(p,l); }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 { ALLOCS.fetch_add(1,Ordering::Relaxed); System.realloc(p,l,n) }
}
#[global_allocator] static ALLOC: CountAlloc = CountAlloc;
#[derive(Clone,Copy,Debug,PartialEq,Eq)] pub struct Color(u32);
impl Color { pub const WHITE: Self=Self(0xffffff); pub fn rgb(r:u8,g:u8,b:u8)->Self{Self((r as u32)<<16|(g as u32)<<8|b as u32)} }
#[derive(Clone,Copy)] pub struct Box64{x0:i64,y0:i64,x1:i64,y1:i64}
impl Box64 {
    pub fn new(x:i32,y:i32,w:i32,h:i32)->Self{Self::wide(x as i64,y as i64,w as i64,h as i64)}
    fn wide(x:i64,y:i64,w:i64,h:i64)->Self{Self{x0:x,y0:y,x1:x+w,y1:y+h}}
    fn intersect(&self,o:&Self)->Self{Self{x0:self.x0.max(o.x0),y0:self.y0.max(o.y0),x1:self.x1.min(o.x1),y1:self.y1.min(o.y1)}}
    fn is_empty(&self)->bool{self.x0>=self.x1||self.y0>=self.y1}
    fn corners(&self)->[i32;4]{[self.x0 as i32,self.y0 as i32,self.x1 as i32,self.y1 as i32]}
}
pub struct Surface{clip:Box64,stack:Vec<Box64>,pixels:Vec<u64>,glyphs:usize,fills:usize}
impl Surface {
    fn new()->Self{Self{clip:Box64::new(0,0,750,550),stack:Vec::new(),pixels:vec![0;750*550],glyphs:0,fills:0}}
    fn width(&self)->usize{750} fn height(&self)->usize{550}
    fn batch(&mut self,f:impl FnOnce(&mut Self)){f(self)}
    fn push_clip(&mut self,b:Box64){self.stack.push(self.clip);self.clip=self.clip.intersect(&b);}
    fn pop_clip(&mut self){self.clip=self.stack.pop().unwrap();}
    fn clip_box(&self)->Box64{self.clip}
    fn fail(&mut self,e:i64){panic!("device error {e}")}
    fn mark(&mut self,_:Box64){}
    fn submit(&mut self,f:impl FnOnce(&mut Context)){f(&mut Context(self));}
    fn rect(&mut self,b:Box64,value:u64){let b=b.intersect(&self.clip);for y in b.y0..b.y1{for x in b.x0..b.x1{self.pixels[y as usize*750+x as usize]=value;}}}
}
#[derive(Clone)]struct Instance{rect:[i32;4],params:[i32;5]}
fn instance(rect:[i32;4],p:&[i32;5])->Instance{Instance{rect,params:*p}}
struct Context<'a>(&'a mut Surface);
impl Context<'_>{fn draw(&mut self,_:u32,c:u32,t:u64,_:u64,_:u64,instances:&[Instance]){for i in instances{self.0.glyphs+=1;let[x0,y0,x1,y1]=i.rect;for y in y0..y1{for x in x0..x1{let sx=i.params[2]+x-i.params[0];let sy=i.params[3]+y-i.params[1];self.0.pixels[y as usize*750+x as usize]=((t+1)<<48)|(u64::from(c)<<16)|((sx as u64+sy as u64)&65535);}}}}}
fn word(c:Color)->u32{c.0}
mod pipeline{pub const COVERAGE:u32=7;}
mod abi{pub mod limits{pub const WORK:u64=4_000_000;}}
mod work{pub fn add(_:u64){}}
mod shapes{use super::*;pub fn fill_rect(f:&mut Surface,x:i32,y:i32,w:i32,h:i32,c:Color){f.fills+=1;f.rect(Box64::new(x,y,w,h),u64::from(c.0));}}
mod surface{pub type Rect=super::Box64;}
mod libgfx{pub mod surface{pub type Rect=super::super::Box64;}}
mod bitmap_font {
    use super::*;
    pub struct Metrics{pub char_width:usize}
    pub fn metrics()->Metrics{Metrics{char_width:7}}
    pub fn draw_text(_: &mut Surface,_:&[u8],_:usize,_:usize,_:Color){panic!("TTF fixture used bitmap");}
}
type Fd=u32;
mod io {pub fn write(_:super::Fd,_:&[u8])->Result<usize,()>{Err(())}}
#[path = "SELECTION_PATH"] mod selection;
mod ttf_font {
    use super::*;
    use std::sync::Arc;
    struct Target(u64);impl Target{fn handle(&self)->u64{self.0}}
    struct Region{x:usize,y:usize,target:Target}
    struct AtlasGlyph{x_offset:i32,y_offset:i32,width:usize,height:usize,region:Option<Region>}
    #[derive(Clone,Copy)]struct Stats{entries:usize,misses:u64}
    pub struct CachedFont{images:Vec<(u16,Arc<AtlasGlyph>)>,misses:u64}
    impl CachedFont{
        pub fn new()->Self{Self{images:Vec::with_capacity(256),misses:0}}
        fn glyph_index(&self,ch:char)->u16{ch as u16}
        fn phase(&self,_:&'static str){}
        fn cache_stats(&self)->Stats{Stats{entries:self.images.len(),misses:self.misses}}
        fn run(&mut self,p:&[(u16,i64)],s:i32,out:impl FnMut(Result<(Arc<AtlasGlyph>,i64),Error>)){self.run_mode(p,s,false,out)}
        fn run_mode(&mut self,p:&[(u16,i64)],_:i32,_:bool,mut out:impl FnMut(Result<(Arc<AtlasGlyph>,i64),Error>)){
            for &(g,x) in p {let image=if let Some((_,a))=self.images.iter().find(|(key,_)|*key==g){a.clone()}else{
                self.misses+=1;let a=Arc::new(AtlasGlyph{x_offset:0,y_offset:0,width:5,height:8,region:Some(Region{x:g as usize*8,y:0,target:Target(1)})});self.images.push((g,a.clone()));a};out(Ok((image,x)));}
        }
    }
    struct Font;impl Font{fn size26(s:f32)->Result<i32,()>{Ok((s*64.)as i32)}}
    #[derive(Debug)]enum Error{Device(i64),Other}
    fn decode_utf8(t:&[u8])->impl Iterator<Item=u32>+'_ {std::str::from_utf8(t).unwrap().chars().map(|c|c as u32)}
    fn glyphs_of(_: &CachedFont,c:impl Iterator<Item=u32>)->impl Iterator<Item=u16>{c.map(|v|v as u16)}
    fn visible(_: &Surface,_:&mut CachedFont,g:Vec<u16>,_:i32,_:i32,_:f32)->(Vec<(u16,i64)>,i32){(g.into_iter().enumerate().map(|(i,g)|(g,i as i64*6)).collect(),0)}
    fn draw_text_bytes(_: &mut Surface,_:&mut CachedFont,_:&[u8],_:i32,_:i32,_:f32,_:Color){panic!("unexpected eviction");}
    fn draw_glyphs_subpixel(_: &mut Surface,_:&mut CachedFont,_:Vec<u16>,_:i32,_:i32,_:f32,_:Color)->i32{panic!("grayscale fixture used LCD");}
    // FONT_CODE
    pub fn errors(){let e=[Error::Device(1),Error::Other];assert_eq!(e.len(),2);}
    pub fn exercise_run(){CachedFont::new().run(&[],640, |_|{});}
    pub fn glyph_id(){assert_eq!(CachedFont::new().glyph_index('a'),97);}
}
use ttf_font::CachedFont;
// TERMINAL_CODE
fn draw(term:&mut TermEmu,fb:&mut Surface,font:&mut CachedFont){term.render(fb,0,48,750,502,6,13,10.,Some(font));}
fn full(term:&mut TermEmu,font:&mut CachedFont)->Surface{
    // A fresh terminal has no painted history; copying the visible VT exercises the
    // same painter as an independent full redraw, rather than its damage decisions.
    let mut copy=TermEmu::new(125,38);
    for y in 0..38 {let mut line=format!("\x1b[{};1H",y+1);for x in 0..125{let c=term.vt.view_cell(x,y);line.push_str(&format!("\x1b[{};{};{}m{}",if c.inverse{7}else{27},if c.underline{4}else{24},if c.bold{1}else{22},c.ch));}copy.vt.feed(line.as_bytes());}
    // Use cursor state for the cursor underline in the independent repaint.
    let(x,y)=term.vt.cursor();copy.vt.feed(format!("\x1b[{};{}H",y+1,x+1).as_bytes());
    if !term.vt.cursor_visible(){copy.vt.feed(b"\x1b[?25l");}
    let mut fb=Surface::new();draw(&mut copy,&mut fb,font);fb
}
fn main(){
    ttf_font::errors();ttf_font::exercise_run();ttf_font::glyph_id();
    let mut term=TermEmu::new(125,38);
    // The launch-each Log Viewer uses bless: 37 rows, truncation to 125 columns,
    // erase-line per row and an inverse status row. A fixed log makes counts stable.
    let mut screen=String::from("\x1b[2J\x1b[H");
    for y in 0..37{screen.push_str(&format!("\x1b[{};1H\x1b[2K[ INFO] kernel::elf: Loaded program {y:02}; image ready; process waiting for input",y+1));}
    screen.push_str("\x1b[38;1H\x1b[7m\x1b[2K /var/log/kernel.log  L:1-37/37  100%  [FOLLOW]\x1b[0m");
    term.feed(screen.as_bytes(),0);
    let mut fb=Surface::new();let mut font=CachedFont::new();
    ALLOCS.store(0,Ordering::Relaxed);let started=std::time::Instant::now();draw(&mut term,&mut fb,&mut font);let elapsed=started.elapsed();let allocations=ALLOCS.load(Ordering::Relaxed);
    println!("cold cells=4750 glyph_draws={} allocations={} host_elapsed_us={}",fb.glyphs,allocations,elapsed.as_micros());
    // A screen of fewer than 256 distinct glyphs needs bulk buffers plus one
    // coverage token per glyph, not several heap allocations per occupied cell.

    let expected=full(&mut term,&mut font);assert_eq!(fb.pixels,expected.pixels);
    fb.glyphs=0;fb.fills=0;term.feed(screen.as_bytes(),0);draw(&mut term,&mut fb,&mut font);
    println!("unchanged glyph_draws={} fills={}",fb.glyphs,fb.fills);
    let unchanged=(fb.glyphs,fb.fills);
    term.feed(b"\x1b[2;10H!\x1b[3;1H\x1b[2K",0);draw(&mut term,&mut fb,&mut font);
    let expected=full(&mut term,&mut font);assert_eq!(fb.pixels,expected.pixels,"erased cells or old cursor survived");
    term.resize(125,38);
    term.selection.press(0,48,selection::Geometry{x:0,y:48,w:6,h:13},(125,38));term.selection.release(24,48,selection::Geometry{x:0,y:48,w:6,h:13},(125,38));term.dirty=true;draw(&mut term,&mut fb,&mut font);
    term.selection.clear();term.dirty=true;draw(&mut term,&mut fb,&mut font);
    let expected=full(&mut term,&mut font);assert_eq!(fb.pixels,expected.pixels,"selection damage survived clearing");
    assert!(allocations<512,"cold terminal frame allocates per cell: {allocations}");
    assert_eq!(unchanged,(0,0),"unchanged output must not repaint the grid");
    // Exercise the remaining production selection methods without suppressions.
    assert!(term.selection.text(&term.vt).is_none());
}
